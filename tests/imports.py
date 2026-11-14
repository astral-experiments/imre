"""Check closed module graphs against CPython and exercise rejected import boundaries."""

import argparse
import gzip
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
ORDERED_DEFINITIONS = (
    "from helper import f\n"
    "def scale(value: int) -> int: return value * 7 + 3\n"
    "print(scale(f(2)))\n"
    "def bound(value: int) -> int: return scale(value)\n"
    "print(bound(f(3)))\n"
)

# The wrong call is never executed. Checked builds must diagnose it in either source order.
LOCALE_CALL = "def typed(n: int) -> int: return n\ndef unused() -> int: return typed({argument})\n"
LOCALE_CHAR_MAX = "from _locale import CHAR_MAX\n" + LOCALE_CALL + "print(CHAR_MAX)\n"
LOCALE_FORMAT = (
    LOCALE_CALL
    + "from _locale import CHAR_MAX\nfrom locale import format_string\n"
    + "print(format_string('%d', CHAR_MAX))\n"
)

RETURNED_ARGUMENT = (
    "def take(value: int) -> int:\n    return value\n\n"
    "def text():\n    return 'wrong'\n"
    "print(take(text()))\n"
    "import opcode\n"
)

WRAPPED_MEASURE = (
    "from functools import wraps\nimport scores\n"
    "def wrapper(func):\n"
    "    @wraps(func)\n"
    "    def invoke(): return func()\n"
    "    return invoke\n"
    "result = wrapper(scores.measure)\n"
    "print(result(), result.__name__, result.__doc__, result.__wrapped__ is scores.measure)\n"
)

REJECTED = {
    "wraps_preserves_imported_return": ({
        "main.py": WRAPPED_MEASURE,
        "scores.py": "def measure() -> int:\n    return 'bad'\n",
    }, "scores.py:2:12: Imre: expected int, got str"),
    "initializer_receives_returned_argument": ({
        "main.py": RETURNED_ARGUMENT,
    }, "main.py:6:12: Imre: expected int, got str"),
    "initializer_imports_completed_value": ({
        "owner.py": "data = 42\n",
        "main.py": "from owner import data\n" + RETURNED_ARGUMENT,
    }, "main.py:7:12: Imre: expected int, got str"),
    "futures_does_not_activate_unused_calendar": ({
        "features.py": "import calendar\nimport concurrent.futures\n",
        "main.py": "import features\ndef scale(x: float) -> float: return 1.5 * x\nprint(scale(2.5))\n",
    }, "requires runtime import initialization"),
    "futures_requires_initializer": ({
        "main.py": "from concurrent.futures import Future\nprint(7)\n",
    }, "concurrent.futures requires runtime import initialization"),
    **{
        f"futures_preserves_first_party_call_{order}": ({
            "features.py": "import concurrent.futures\n",
            "scores.py": (
                "def take(n: int) -> int: return n\n"
                "def score(debug: bool) -> int:\n"
                "    if debug: return take('x')\n"
                "    return take(7)\n"
            ),
            "main.py": f"{imports}\nprint(scores.score(False))\n",
        }, "scores.py:3:27: Imre: expected int, got str")
        for order, imports in enumerate(("import features, scores", "import scores, features"))
    },
    "local_source_loader_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from scores import take\n"
            "def loader_type():\n"
            "    from importlib.abc import SourceLoader\n"
            "    return SourceLoader\n"
            "print(take('x'))\n"
        ),
    }, "invalid-argument-type"),
    "unused_source_loader_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from importlib.abc import SourceLoader as Loader\n"
            "from scores import take\n"
            "def unused() -> int: return take('x')\n"
            "print(take(7))\n"
        ),
    }, "invalid-argument-type"),
    "source_loader_subclass_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "rules.py": (
            "from importlib.abc import SourceLoader\n"
            "class MemoryLoader(SourceLoader):\n"
            "    def get_filename(self, fullname): return 'rules.py'\n"
            "    def get_data(self, path): return b'value = 42'\n"
        ),
        "main.py": (
            "from rules import MemoryLoader\n"
            "from scores import take\n"
            "def unused() -> int: return take('x')\n"
            "print(MemoryLoader().get_filename('rules'))\n"
        ),
    }, "invalid-argument-type"),
    "unused_reload_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from importlib import reload\n"
            "from scores import take\n"
            "def unused() -> int: return take('x')\n"
            "print(take(7))\n"
        ),
    }, "expected int, got str"),
    "reload_needs_a_module_object": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "import importlib, scores\n"
            "def unused() -> None: importlib.reload(scores)\n"
            "print(scores.take(7))\n"
        ),
    }, "importlib.reload requires native Python object operations"),
    "decimal_float_preserves_typed_call": ({
        "main.py": (
            "from decimal import Decimal\n"
            "def convert(value: Decimal) -> float: return float(value)\n"
            "def typed(n: int) -> int: return n\n"
            "def unused() -> int: return typed('wrong')\n"
            "print(convert(Decimal('1.25')))\n"
        ),
    }, "invalid-argument-type"),
    "clock_lookup_preserves_typed_call": ({
        "main.py": "import time\n" + HELPER + "f('wrong')\nprint(getattr(time, 'monotonic_ns')())\n",
    }, "expected int, got str"),
    "unused_strptime_preserves_typed_call": ({
        "main.py": "from time import strptime\n" + HELPER + "f('wrong')\n",
    }, "expected int, got str"),
    "frozen_importer_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from importlib.machinery import FrozenImporter\n"
            "from scores import take\n"
            "if False: FrozenImporter.find_spec('stat')\n"
            "print(take('x'))\n"
        ),
    }, "invalid-argument-type"),
    "builtin_importer_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from importlib.machinery import BuiltinImporter\n"
            "from scores import take\n"
            "if False: BuiltinImporter.find_spec('errno')\n"
            "print(take('x'))\n"
        ),
    }, "invalid-argument-type"),
    "methodcaller_preserves_typed_call": ({
        "main.py": (
            "from operator import methodcaller\n"
            "def typed(n: int) -> int: return n\n"
            "print(methodcaller('upper')('ok'))\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "unused_is_tracked_preserves_typed_call": ({
        "main.py": "from gc import is_tracked\n" + HELPER + "f('wrong')\n",
    }, "expected int, got str"),
    "later_is_tracked_preserves_missing_argument": ({
        "main.py": "from gc import is_tracked\n" + HELPER + "f()\nprint(is_tracked([]))\n",
    }, "missing required argument"),
    "unused_isclose_preserves_typed_call": ({
        "main.py": (
            "from math import isclose\n"
            "def typed(n: int) -> int: return n\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "isclose_call_preserves_typed_call": ({
        "main.py": (
            "from math import isclose\n"
            "def typed(n: int) -> int: return n\n"
            "print(isclose(1, 1))\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "unused_float_info_preserves_typed_call": ({
        "main.py": (
            "from sys import float_info\n"
            "def typed(n: int) -> int: return n\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "float_info_field_preserves_typed_call": ({
        "main.py": (
            "from sys import float_info\n"
            "def typed(n: int) -> int: return n\n"
            "print(float_info.mant_dig)\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "generator_local_keeps_typed_call": ({
        "helper.py": (
            "def values():\n"
            "    def pair(left, right): return left + right\n"
            "    yield pair(2, 3)\n"
            "def typed(n: int) -> int: return n\n"
        ),
        "main.py": "from helper import values, typed\nprint(next(values()))\ntyped('wrong')\n",
    }, "invalid-argument-type"),
    "docfile_suite_preserves_typed_call": ({
        "main.py": (
            "from doctest import DocFileSuite\n"
            "def typed(n: int) -> int: return n\n"
            "DocFileSuite()\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "unittest_method_skip_preserves_typed_call": ({
        "main.py": (
            "import unittest\n"
            "class Case(unittest.TestCase):\n"
            "    @unittest.skipUnless(False, 'feature required')\n"
            "    def test_body(self): pass\n"
            "    @staticmethod\n"
            "    @staticmethod\n"
            "    def extra(): pass\n"
            "def typed(n: int) -> int: return n\n"
            "typed('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "runtime_method_skip_leaves_other_class_checked": ({
        "main.py": (
            "import unittest\n"
            "class Case(unittest.TestCase):\n"
            "    @unittest.skipUnless(False, 'feature required')\n"
            "    def test_body(self): pass\n"
            "class Other:\n"
            "    @staticmethod\n"
            "    @staticmethod\n"
            "    def extra(): pass\n"
            "print(Other.extra())\n"
        ),
    }, "method decorators cannot be stacked"),
    "leading_mixin_preserves_assignment_check": ({
        "helper.py": "def invalid() -> int:\n    value: int = 'wrong'\n    return value\n",
        "main.py": (
            "from helper import invalid\n"
            "class Methods:\n    def extra(self) -> int: return 1\n"
            "class Storage:\n    def __init__(self, n: int): self.x = n\n"
            "class Child(Methods, Storage): pass\n"
            "print(Child(3).extra())\n"
        ),
    }, "expected int, got str"),
    "second_base_has_storage": ({
        "main.py": (
            "class Left:\n    def __init__(self, x: int): self.left = x\n"
            "class Right:\n    def __init__(self, y: int): self.right = y\n"
            "class Both(Left, Right): pass\n"
        ),
    }, "multiple independent base classes"),
    "math_lcm_preserves_typed_call": ({
        "main.py": (
            "from math import lcm\n"
            "def typed(n: int) -> int: return n\n"
            "print(lcm(6, 10))\n"
            "typed('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "math_constants_preserve_typed_call": ({
        "main.py": (
            "from math import e, inf, nan, pi, tau\n"
            "def typed(value: int) -> int: return value\n"
            "print(inf, pi)\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "custom_dir_preserves_typed_call": ({
        "main.py": (
            "class Subject:\n"
            "    def __dir__(self): return ['value']\n"
            "def typed(n: int) -> int: return n\n"
            "dir(Subject())\n"
            "typed('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "inspect_isdatadescriptor_preserves_typed_call": ({
        "main.py": (
            "from inspect import isdatadescriptor\n"
            "def typed(n: int) -> int: return n\n"
            "print(isdatadescriptor(property()))\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "inspect_classify_class_attrs_preserves_typed_call": ({
        "main.py": (
            "from inspect import classify_class_attrs\n"
            "class Subject: pass\n"
            "def typed(n: int) -> int: return n\n"
            "classify_class_attrs(Subject)\n"
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
    "optional_probe_with_additional_binding": ({
        "main.py": "from probe import other\n",
        "probe.py": (
            "try:\n    import sys\n    result = (other := sys.maxsize)\n"
            "except (ImportError, AttributeError):\n    result = 0\n"
            "import os\n"
        ),
    }, "imports must precede executable module statements"),
    "optional_probe_checks_typed_calls": ({
        "main.py": "from probe import users\nprint(users)\n",
        "probe.py": (
            "def take(value: int) -> int: return value\n"
            "try:\n    import pwd\n    users = [take('wrong') for u in pwd.getpwall()]\n"
            "except (ImportError, AttributeError):\n    users = []\n"
            "import sys\n"
        ),
    }, "invalid-argument-type"),
    "optional_probe_checks_missing_argument": ({
        "main.py": "from probe import result\nprint(result)\n",
        "probe.py": (
            "def take(value: int) -> int: return value\n"
            "try:\n    import sys\n    result = take()\nexcept ImportError:\n    result = 0\n"
        ),
    }, "missing-argument"),
    "optional_from_keeps_typed_call_check": ({
        "main.py": (
            "try:\n    from os.path import basename as leaf\nexcept ImportError:\n    leaf = None\n"
            "import sys\ndef typed(n: int) -> int: return n\nprint(typed('wrong'))\n"
        ),
    }, "expected int, got str; annotations are enforced contracts"),
    "optional_from_rejects_stdlib_shadow": ({
        "main.py": (
            "try:\n    from concurrent.futures import Future\nexcept ImportError:\n    Future = None\n"
            "import sys\n"
        ),
        "concurrent/__init__.py": "",
        "concurrent/futures.py": "Future = 42\n",
    }, "standard-library shadowing"),
    "optional_from_unmatched_fallback": ({
        "main.py": (
            "try:\n    from os.path import basename\nexcept ImportError:\n    other = None\n"
            "import sys\n"
        ),
    }, "imports must precede executable module statements"),
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
    "locale_char_max_keeps_unused_function_checked": ({
        "main.py": LOCALE_CHAR_MAX.format(argument="'wrong'"),
    }, "expected int, got str"),
    "locale_formatting_keeps_earlier_function_checked": ({
        "main.py": LOCALE_FORMAT.format(argument="'wrong'"),
    }, "expected int, got str"),
    "unused_randbytes_preserves_typed_call": ({
        "main.py": (
            "from random import randbytes\n"
            "def typed(n: int) -> int: return n\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "email_message_from_bytes_preserves_typed_call": ({
        "main.py": (
            "from email import message_from_bytes\n"
            "def typed(n: int) -> int: return n\n"
            "message_from_bytes(b'body')\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "randbytes_call_preserves_typed_call": ({
        "main.py": (
            "from random import randbytes\n"
            "def typed(n: int) -> int: return n\n"
            "randbytes(5)\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "fma_preserves_typed_arguments": ({
        "main.py": (
            "from math import fma\n"
            "def typed(n: int) -> int: return n\n"
            "fma(1.0, 2.0, 3.0)\n"
            "typed('wrong')\n"
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
    "glob_preserves_typed_call": ({
        "main.py": "import glob\nfrom scores import score\nprint(glob.translate('*.py'), score)\n",
        "scores.py": "def take(n: int) -> int: return 7\nscore = take('x')\n",
    }, "expected int, got str"),
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
    "source_method_factory_checks_calls": ({
        "helper.py": HELPER,
        "main.py": (
            "import helper\n"
            "class Case:\n"
            "    @helper.f('wrong')\n"
            "    def method(self): pass\n"
        ),
    }, "invalid-argument-type"),
    "method_decorator_must_be_callable": ({
        "helper.py": HELPER,
        "main.py": (
            "import helper\n"
            "class Case:\n"
            "    @helper.f(7)\n"
            "    def method(self): pass\n"
        ),
    }, "call-non-callable"),
    "shutdown_keeps_checked_initializer": ({
        "main.py": (
            "from logging import shutdown as finish\nresult = finish()\n"
            "invalid: int = 'wrong'\nprint(invalid)\n"
        ),
    }, "expected int"),
    "shutdown_with_supplied_weak_refs_requires_object_imports": ({
        "main.py": (
            "import logging, weakref\nhandler = logging.NullHandler()\n"
            "logging.shutdown(handlerList=[weakref.ref(handler)])\n"
        ),
    }, "native lowering of logging.shutdown supports only the no-argument form"),
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
    "handler_names_preserves_assignment": ({
        "main.py": (
            "from logging import getHandlerNames\ndef run() -> None:\n"
            "    names = getHandlerNames()\n    print('handler' in names)\n"
            "    value: int = 'wrong'\n    print(value)\nrun()\n"
        ),
    }, "expected int, got str"),
    "pkgutil_resolve_name_preserves_checked_calls": ({
        "main.py": (
            "from pkgutil import resolve_name\n"
            "def take(n: int) -> int:\n    return n\n"
            "print(resolve_name('builtins:int'), take('wrong'))\n"
        ),
    }, "invalid-argument-type"),
    "platform_iot_preserves_checked_calls": ({
        "main.py": (
            "import helper\ndef typed(value: int) -> int:\n    return value + 1\n"
            "print(helper.win32_is_iot(), typed('wrong'))\n"
        ),
        "helper.py": "from platform import win32_is_iot\n",
    }, "invalid-argument-type"),
    "unused_queue_listener_preserves_assignment": ({
        "main.py": (
            "from logging.handlers import QueueListener\ndef run() -> None:\n"
            "    value: int = 'wrong'\n    print(value)\nrun()\n"
        ),
    }, "expected int, got str"),
    "saved_queue_handler_preserves_checked_calls": ({
        "main.py": (
            "from logging.handlers import QueueHandler as Handler\ndef take(n: int) -> None:\n    print(n)\n"
            "def run() -> None:\n    constructor = Handler\n    print(constructor.__name__)\n"
            "    take('wrong')\nrun()\n"
        ),
    }, "invalid-argument-type"),
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
    "struct_clearcache_preserves_typed_call": ({
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from struct import _clearcache as clear\nfrom scores import take\n"
            "def unused() -> int: return take('x')\nclear()\n"
        ),
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
    "unused_import_builtin_keeps_checked_import": ({
        "main.py": '__import__("helper")\nif False:\n    unused = __import__\n',
        "helper.py": HELPER,
    }, "unsupported"),
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
    "inspect_descriptor_doc_preserves_typed_call": ({
        "main.py": (
            "from inspect import getdoc, ismemberdescriptor\n"
            "def typed(n: int) -> int: return n\n"
            "getdoc(int)\n"
            "ismemberdescriptor(int)\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "float_fromhex_preserves_uncalled_argument_check": ({
        "main.py": (
            "def typed(n: int) -> int: return n\n"
            "def helper() -> float: return float.fromhex('0x1p0')\n"
            "def unused() -> int: return typed('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "namedtuple_descriptor_preserves_typed_call": ({
        "main.py": (
            "from collections import namedtuple\n"
            "Point = namedtuple('Point', 'x y')\n"
            "field = Point.x\n"
            "def typed(n: int) -> int: return n + 1\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "namedtuple_descriptor_shadowed_by_parameter": ({
        "main.py": (
            "from collections import namedtuple\n"
            "Point = namedtuple('Point', 'x y')\n"
            "def read(Point: int) -> int: return Point.x\n"
            "print(read(5))\n"
        ),
    }, "rebinding a builtin, definition, or imported module"),
    "namedtuple_descriptor_shadowed_by_comprehension_target": ({
        "main.py": (
            "from collections import namedtuple\n"
            "Point = namedtuple('Point', 'x')\n"
            "print([1 for _ in [0] if Point.x for Point in [3]])\n"
        ),
    }, "comprehension local may be uninitialized on this path"),
    "namedtuple_module_shadowed_by_parameter": ({
        "points.py": "from collections import namedtuple\nPoint = namedtuple('Point', 'x y')\n",
        "main.py": (
            "import points\n"
            "def read(points: int) -> int: return points.Point.x\n"
            "print(read(5))\n"
        ),
    }, "rebinding a builtin, definition, or imported module"),
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
    "traceback_print_list_preserves_typed_call": ({
        "main.py": (
            "from traceback import print_list\n"
            "def typed(n: int) -> int: return n\n"
            "print_list([])\n"
            "typed('wrong')\n"
        ),
    }, "invalid-argument-type"),
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
    "unused_lzma_exports_preserve_typed_call": ({
        "main.py": (
            "from lzma import decompress, LZMAError, FORMAT_AUTO, FORMAT_XZ, FORMAT_ALONE\n"
            "def typed(n: int) -> int: return n\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "lzma_decompress_preserves_typed_call": ({
        "main.py": (
            "from lzma import decompress\n"
            "def typed(n: int) -> int: return n\n"
            "decompress(b']\\x00\\x00\\x80\\x00\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\x00\\x83\\xff\\xfb\\xff\\xff\\xc0\\x00\\x00\\x00')\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
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
    "child_reexport_keeps_call_diagnostic": ({
        "pkg/__init__.py": "from .label import label\n",
        "pkg/label.py": "label = 'version'\n",
        "scores.py": "def take(x: int) -> int: return x\ndef unused() -> int: return take('x')\n",
        "main.py": "from pkg import label\nimport scores\nfrom importlib import import_module\nprint(import_module(label))\n",
    }, "expected int, got str"),
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
    "invalid_call_after_imported_initializer": ({
        "feature.py": "def require(enabled: bool):\n    if not enabled: raise RuntimeError('unavailable')\n",
        "main.py": "import feature\nfeature.require(True)\ndef typed(value: int) -> int: return value\ntyped('bad')\n",
    }, "expected int, got str"),
    "later_definition_keeps_external_signature": ({
        "main.py": ORDERED_DEFINITIONS,
        "helper.py": HELPER + "def dormant(value: complex) -> complex: return value + 1j\n",
    }, "unsupported exact type annotation"),
    "later_definition_keeps_scalar_argument": ({
        "main.py": ORDERED_DEFINITIONS + "scale('bad')\n",
        "helper.py": HELPER,
    }, "expected int, got str"),
    "missing_installed_root": ({
        "main.py": "from invoice_rates.lookup import rate\nprint(rate(7))\n",
    }, "--extension-path DIRECTORY"),
    "missing_namespace_child": ({"main.py": "import package.missing\n", "package/child.py": HELPER}, "cannot resolve first-party module"),
    "namespace_preserves_annotation": ({
        "main.py": "import helper\n",
        "space/child.py": "value = 3\n",
        "helper.py": "from space.child import value\ndef result() -> int:\n    return 'wrong'\nresult()\n",
    }, "expected int, got str"),
    "previously_bound_child": ({
        "main.py": "import package\n",
        "package/__init__.py": "from . import token\nfrom .token import token\n",
        "package/token.py": "token = 42\n",
    }, "cannot be rebound"),
    "child_overwrites_definition": ({"main.py": "import package.child\n", "package/__init__.py": "def child() -> int:\n    return 1\n", "package/child.py": HELPER}, "overwrite"),
    "entry_self_import": ({"main.py": "import main\n"}, "entry module"),
    "reserved_alias": ({"main.py": "from helper import f as int\n", "helper.py": HELPER}, "reserved"),
    "normalized_reserved_alias": ({"main.py": "from helper import f as ℕone\n", "helper.py": HELPER}, "reserved"),
    "stdlib_shadow": ({"main.py": "import sys\n", "sys.py": HELPER}, "standard-library"),
    "operator_provider_shadow": ({
        "main.py": "import consumer\n",
        "consumer.py": "from _operator import _compare_digest\n",
        "_operator.py": HELPER,
    }, 'shadowing is unsupported: "_operator"'),
    "py_compile_shadow": ({"main.py": "import py_compile\n", "py_compile.py": HELPER}, 'shadowing is unsupported: "py_compile"'),
    "fractions_shadow": ({
        "main.py": "import fractions\n", "fractions.py": HELPER,
    }, 'shadowing is unsupported: "fractions"'),
    "fractions_dependency_shadow": ({
        "main.py": "import fractions\n", "numbers.py": HELPER,
    }, 'shadowing is unsupported: "numbers"'),
    "ntpath_shadow": ({
        "main.py": "from helper import parts\nprint(parts())\n",
        "helper.py": "import ntpath\ndef parts(): return ntpath.splitdrive('C:/a')\n",
        "ntpath.py": HELPER,
    }, 'shadowing is unsupported: "ntpath"'),
    "token_shadow": ({"main.py": "import token\n", "token.py": HELPER}, 'shadowing is unsupported: "token"'),
    "email_address_parser_unknown_name": ({
        "main.py": "from email._header_value_parser import missing_parser\n",
    }, "unsupported sealed standard-library export"),
    "unused_email_address_parser_keeps_types": ({
        "main.py": "from email._header_value_parser import get_addr_spec\n"
                   "def identity(n: int) -> int: return n\nidentity('wrong')\n",
    }, "expected int, got str"),
    "opcode_shadow": ({"main.py": "import _opcode\n", "_opcode.py": HELPER}, "shadowing"),
    "inspect_shadow_invalid_return": ({
        "main.py": "from inspect import f\nprint(f(2))\n",
        "inspect.py": HELPER + "\ndef uncalled() -> int:\n    return 'wrong'\n",
    }, "expected int, got str"),
    "opcode_unknown_operation": ({"main.py": "from _opcode import not_an_opcode_operation\n"}, "not_an_opcode_operation"),
    "opcode_typed_argument": ({"main.py": "import _opcode as op\ndef text(x: str) -> None:\n    print(x)\ntext(op.ENABLE_SPECIALIZATION)\n"}, "expected str, got int"),
    "unstubbed_builtin_shadow": ({"main.py": "import _types\n", "_types.py": HELPER}, "standard-library"),
    "imported_surrogate": ({"main.py": IMPORT, "helper.py": '"\\ud800"\n' + HELPER}, "surrogate"),
    "imported_encoding": ({"main.py": IMPORT, "helper.py": "# coding: latin1\n" + HELPER}, "encoding"),
}

ACCEPTED = {
    "wraps_uses_source_defaults": {
        "main.py": WRAPPED_MEASURE,
        "scores.py": 'def measure() -> int:\n    """score doc"""\n    return 7\n',
    },
    "unreached_initializer_local": {
        "main.py": "import child\nprint('after child')\n",
        "child.py": (
            "for item in range(0):\n    pass\n"
            "print('started')\nraise SystemExit(0)\nprint(item)\n"
        ),
    },
    "bisect_obeys_the_target_finder": {
        "_bisect.py": "def bisect_right(a, x): return 1234\n",
        "main.py": (
            "import bisect\nimport sys\n"
            "result = bisect.bisect_right([1, 2, 2], 2)\n"
            "print(result == (3 if '_bisect' in sys.builtin_module_names else 1234))\n"
        ),
    },
    "bisect_from_its_module_initializer": {
        "main.py": (
            "import bisect\nfrom bisect import bisect_left\n"
            "right = bisect.bisect_right\n"
            "values = [1, 2, 2, 4]\n"
            "print(bisect_left(values, 2), right(values, 2), bisect.bisect is right)\n"
            "print(right(values, 2, hi=-1), right.__module__)\n"
        ),
    },
    "definition_after_execution": {"main.py": ORDERED_DEFINITIONS, "helper.py": HELPER},
    "class_after_execution": {
        "main.py": (
            "value = 1\nprint(value)\n"
            "class Box:\n    field = value\n"
            "value = 2\nprint(Box.field, value)\n"
        ),
    },
    "sdk_dependency_keeps_unused_frozen_shadow_dormant": {
        "main.py": (
            "import sys\n"
            "def archive_type():\n"
            "    import tarfile\n"
            "    return tarfile.TarFile\n"
            "print('tarfile' in sys.modules)\n"
        ),
        **dict.fromkeys(
            ("stat.py", "zipimport.py"),
            "def amount(value: int) -> int: return value\ndef dormant(): return amount('x')\n",
        ),
    },
    "sdk_import_after_provider_hint": {
        "main.py": (
            "import sys\n"
            "import shutil\n"
            "import reader\n"
            "print(reader.archive is sys.modules['tarfile'])\n"
        ),
        "reader.py": "import tarfile as archive\n",
    },
    "function_type_for_selected_callback": {
        "handlers.py": (
            "import sys\n\n"
            "def increment(n: int) -> int:\n"
            "    return n + 1\n\n"
            "def double(n: int) -> int:\n"
            "    return n * 2\n\n"
            "if len(sys.argv) > 1:\n"
            "    selected = double\n"
            "else:\n"
            "    selected = increment\n"
        ),
        "main.py": (
            "from handlers import selected\n"
            "from types import FunctionType\n\n"
            "copied = FunctionType(selected.__code__, globals(), \"copied\")\n"
            "print(type(copied) is FunctionType, copied is not selected, copied(40))\n"
        ),
    },
    "imported_tabs": {"main.py": IMPORT, "helper.py": "def f(value: int) -> int:\n\treturn value\n"},
    "inspect_shadow": {
        "main.py": "from inspect import f\nprint(f(2))\n",
        "inspect.py": HELPER,
    },
    "methodcaller_module_alias": {
        "main.py": "import operator as op\nprint(op.methodcaller('upper')('ok'))\n",
    },
    "methodcaller_from_alias": {
        "main.py": "from operator import methodcaller as call\nprint(call('replace', 'a', 'b')('abc'))\n",
    },
    "unreached_local_source_loader": {
        "scores.py": "def take(value: int) -> int: return value\n",
        "main.py": (
            "from scores import take\n"
            "def loader_type():\n"
            "    from importlib.abc import SourceLoader\n"
            "    return SourceLoader\n"
            "print(take(3))\n"
        ),
    },
    "doctest_qualified_exception": {
        "events_module.py": (
            '"""\n>>> raise Problem("sample event")\nTraceback (most recent call last):\n'
            '...\nevents_module.Problem: sample event\n"""\n'
            "class Problem(Exception): pass\n"
        ),
        "main.py": (
            "import doctest\nimport events_module\n"
            "for case in doctest.DocTestSuite(events_module):\n    case.debug()\n    print(case.id())\n"
        ),
    },
    "locale_typed_char_max": {"main.py": LOCALE_CHAR_MAX.format(argument="1")},
    "locale_formats_typed_char_max": {"main.py": LOCALE_FORMAT.format(argument="1")},
    "generator_local_argument_binding": {
        "helper.py": (
            "def values():\n"
            "    def pair(left, right): return left + right\n"
            "    yield pair(2, 3)\n"
            "    yield pair(2, 3, 4)\n"
        ),
        "main.py": (
            "from helper import values\n"
            "gen = values()\n"
            "print(next(gen))\n"
            "try: next(gen)\n"
            "except TypeError as error: print(error)\n"
        ),
    },
    "imported_leading_method_mixin": {
        "helper.py": (
            "OFFSET = 7\n"
            "class Methods:\n"
            "    def extra(self) -> int: return OFFSET\n"
        ),
        "main.py": (
            "from helper import Methods\n"
            "class Storage:\n"
            "    def __init__(self, n: int): self.number = n\n"
            "    def value(self) -> int: return self.number\n"
            "class Child(Methods, Storage):\n"
            "    def total(self) -> int: return super().extra() + super().value()\n"
            "item = Child(5)\n"
            "print(item.total())\n"
            "print(isinstance(item, Methods))\n"
            "print(isinstance(item, Storage))\n"
        ),
    },
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
    "nested_operator_compare_digest": {
        "main.py": "from consumer import equal\nprint(equal(b'ab', b'ab'), equal(b'ab', b'ac'))\n",
        "consumer.py": "from _operator import _compare_digest as equal\n",
    },
    "lzma_formats_and_error": {
        "main.py": (
            "from lzma import FORMAT_AUTO, FORMAT_XZ, FORMAT_ALONE, LZMAError as Error\n"
            "assert FORMAT_AUTO == 0 and FORMAT_XZ == 1 and FORMAT_ALONE == 2\n"
            "try:\n"
            "    raise Error('bad stream')\n"
            "except Error as exc:\n"
            "    print(str(exc))\n"
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
    "ordered_entry_class": {
        "feature.py": "def initialize(): print('initialize')\n",
        "main.py": (
            "import feature\nfeature.initialize()\n"
            "if hasattr(feature, 'initialize'):\n    class Available: pass\n"
            "if hasattr(feature, 'missing'):\n    class Missing: pass\n"
            "class Last: pass\nprint(Available.__name__, Last.__name__)\n"
            "try: Missing()\nexcept NameError: print('absent')\n"
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
    "rebound_import_surrogate_file_origin": {
        "pkg/__init__.py": "",
        "pkg/middle/__init__.py": "",
        "pkg/middle/leaf.py": "answer = 41\n",
        "main.py": (
            "import pyexpat\nimport pkg.middle.leaf as physical\nimport pkg\n"
            "class Spec:\n    has_location = True\n"
            "    @property\n    def _initializing(self): raise RuntimeError('unexpected read')\n"
            "class Receiver: pass\n"
            "spec = Spec()\nspec.origin = __file__.rsplit('/', 1)[0] + '/local-' + chr(0xdcff) + '.py'\n"
            "view = Receiver()\nview.__name__ = 'json'\nview.__spec__ = spec\npkg.middle = view\n"
            "try:\n    import pkg.middle.leaf as selected\n"
            "except ImportError as error:\n    print(error.path is spec.origin, physical.answer)\n"
        ),
    },
    "nonmodule_import_error_recovery": {
        "pkg/__init__.py": "",
        "pkg/middle/__init__.py": "",
        "pkg/middle/last.py": "answer = 41\n",
        "main.py": (
            "from importlib import import_module\n"
            "pkg = import_module('pkg')\n"
            "physical = import_module('pkg.middle.last')\n"
            "class NamespaceView: pass\n"
            "pkg.middle = NamespaceView()\n"
            "try:\n"
            "    import pkg.middle.last as selected\n"
            "except ImportError as exc:\n"
            "    print(str(exc), exc.name, exc.path, exc.name_from)\n"
            "    selected = import_module('pkg.middle.last')\n"
            "print(selected is physical, selected.answer)\n"
        ),
    },
    "boxed_reexported_child": {
        "pkg/__init__.py": "from .label import label\n",
        "pkg/label.py": "label = 73\nphysical = 41\n",
        "main.py": "from importlib import import_module\nimport pkg.label as alias\nprint(alias)\nprint(import_module('pkg.label').physical)\n",
    },
    "initializing_child_after_reexport": {
        "banner/__init__.py": "from .version import version\n",
        "banner/version.py": "version = 1\n",
        "pkg/__init__.py": "",
        "pkg/middle/__init__.py": "import pkg.middle.leaf as first\nanswer = first.value\n",
        "pkg/middle/leaf.py": "value = 37\n",
        "main.py": "import banner\nimport pkg.middle\nprint(pkg.middle.answer)\n",
    },
    "reexported_child_module": {
        "pkg/__init__.py": "from .middle import middle\n",
        "pkg/middle/__init__.py": "from .. import alternate as middle\n",
        "pkg/middle/last.py": "answer = 41\n",
        "pkg/alternate/__init__.py": "from . import last\n",
        "pkg/alternate/last.py": "answer = 73\n",
        "main.py": "import pkg.middle.last as alias\nfrom pkg.middle.last import answer\nprint(alias.answer)\nprint(answer)\n",
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
    "fractions_indirect": {
        "main.py": "from helper import total\nprint(total())\n",
        "helper.py": (
            "import fractions\n"
            "from numbers import Rational\n"
            "def total():\n"
            "    assert fractions.Fraction(-3, 2).limit_denominator(1) == -2\n"
            "    assert fractions.Fraction(-355, 113).limit_denominator(10) == fractions.Fraction(-22, 7)\n"
            "    value = fractions.Fraction(6, -8) + fractions.Fraction(1, 4)\n"
            "    return isinstance(value, Rational), value.numerator, value.denominator\n"
        ),
    },
    "platform_iot_indirect": {
        "main.py": "import helper\nprint(helper.win32_is_iot())\n",
        "helper.py": "from platform import win32_is_iot\n",
    },
    "ntpath_indirect": {
        "main.py": "from helper import paths\nprint(paths())\n",
        "helper.py": (
            "import ntpath\n"
            "from ntpath import splitdrive\n"
            "def paths():\n"
            "    return ntpath.__name__, splitdrive('C:/base'), ntpath.join('C:', 'a', 'b')\n"
        ),
    },
    "optional_import_in_dependency": {
        "helper.py": (
            "try:\n    import sys as provider\nexcept ImportError:\n    provider = None\n"
            "import child\n"
            "assert provider is not None\n"
            "def value() -> int: return 7\n"
        ),
        "child.py": (
            "from importlib import import_module\n"
            "parent = import_module('helper')\n"
            "print(parent.provider is not None)\n"
        ),
        "main.py": "from helper import value\nprint(value())\n",
    },
    "optional_probe_with_dormant_call": {
        "main.py": (
            "from provider import ready\nfrom io import BytesIO\n"
            "print(BytesIO(b'x').getbuffer().nbytes)\nprint(ready())\n"
        ),
        "provider.py": (
            "def take(x: int) -> int: return x\ndef dormant(): return take()\n"
            "try:\n    import sys as optional\n    detail = optional.maxsize\n"
            "except ImportError:\n    detail = 0\n"
            "def ready() -> int: return 42\n"
        ),
    },
    "optional_stdlib_probe_in_dependency": {
        "main.py": "from pkg.probe import witness\nprint(witness.checked)\n",
        "pkg/__init__.py": "",
        "pkg/probe.py": (
            "import sys\n"
            "try:\n    import pwd as accounts\n    users = [u.pw_uid for u in accounts.getpwall()]\n"
            "except (ImportError, AttributeError):\n    users = []\n"
            "try:\n    import math as numbers\n    from math import pi, missing_value\n"
            "except ImportError:\n    numbers = None\n    pi = missing_value = sys.maxsize\n"
            "from . import witness\n"
        ),
        "pkg/witness.py": (
            "from importlib import import_module\n"
            "probe = import_module('pkg.probe')\n"
            "checked = (isinstance(probe.users, list), probe.numbers is None, probe.pi == probe.missing_value)\n"
        ),
    },
    "optional_stdlib_from_in_dependency": {
        "main.py": "from pkg.consumer import witness\nprint(witness.checked)\n",
        "pkg/__init__.py": "",
        "pkg/consumer.py": (
            "try:\n    from os.path import basename as leaf\nexcept ImportError:\n    leaf = None\n"
            "try:\n    from os.path import nonexistent_name\nexcept ImportError:\n    nonexistent_name = None\n"
            "from . import witness\n"
        ),
        "pkg/witness.py": (
            "from importlib import import_module\n"
            "consumer = import_module('pkg.consumer')\n"
            "checked = (consumer.leaf('a/b'), consumer.nonexistent_name is None)\n"
        ),
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
        "main.py": "import paths\nimport prepare\nimport codes\nimport computed\nimport collection_abcs\n",
        "collection_abcs.py": (
            "from collections.abc import *\nimport collections.abc as _abc\n"
            "_names = [n for n in globals() if not n.startswith('_')]\n"
            "assert _names == _abc.__all__\n"
            "for _name in _names:\n    assert globals()[_name] is getattr(_abc, _name)\n"
            "print(_names)\n"
        ),
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
    "wildcard_binding_release": {
        "main.py": "from provider import *\nprint(_first, second)\n",
        "provider.py": (
            "import sys\napp = sys.modules['__main__']\n"
            "class Key(str): pass\n"
            "class Pending:\n    def __del__(self): print('second' in app.__dict__)\n"
            "app.__dict__[Key('_first')] = Pending()\n"
            "__all__ = ['_first', 'second']\n_first = 17\nsecond = 37\n"
        ),
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
    "unused_glob_module": {
        "main.py": "import glob\ndef weight(n: int) -> int: return n * 3 + 2\nprint(weight(13))\n",
    },
    "type_checking_runtime_export": {
        "main.py": (
            "import helper\nfrom helper import Layer, token as selected\n"
            "print(selected is helper.token, selected[0])\n"
            "print(Layer is helper.Layer, Layer.__name__)\n"
        ),
        "helper.py": (
            "import typing as t\n"
            "def initialize():\n    print('initialized')\n    return [41]\n"
            "if t.TYPE_CHECKING:\n    from unavailable_at_runtime import token\n"
            "else:\n    token = initialize()\n    class Layer(dict): pass\n"
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
    "struct_clearcache_evaluates_rejected_arguments": {
        "main.py": (
            "from struct import _clearcache\n"
            "def touch(n: int) -> int:\n    print(n)\n    return n\n"
            "try:\n    _clearcache(touch(1), key=touch(2))\n"
            "except TypeError as exc:\n    print(str(exc))\n"
        ),
    },
    "struct_clearcache_with_typed_call": {
        "scores.py": "def take(value: int) -> int: return value + 1\n",
        "main.py": (
            "import struct\nfrom struct import _clearcache as clear\nfrom scores import take\n"
            "assert struct._clearcache() is None and clear() is None\nprint(take(6))\n"
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
    "marshal_shared_roundtrip": {
        "main.py": (
            "from marshal import dumps, loads\n"
            "print(loads(dumps(42)))\n"
            "shared = [loads(b'i*\\x00\\x00\\x00')]\n"
            "value = (shared, shared)\n"
            "restored = loads(dumps(value))\n"
            "print(restored, restored[0] is restored[1])\n"
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
    "module_docstring_store_frame": {
        "main.py": (
            "import sys\n"
            "import leaf\n"
            "\n"
            "class Key(str):\n"
            "    __hash__ = str.__hash__\n"
            "\n"
            "    def __eq__(self, other):\n"
            "        frame = sys._getframe(1)\n"
            "        print(\"binding\", frame.f_globals[\"__name__\"], frame.f_lineno)\n"
            "        return str.__eq__(self, other)\n"
            "\n"
            "del leaf.__doc__\n"
            "leaf.__dict__[Key(\"__doc__\")] = 41\n"
            "leaf.__spec__.loader.exec_module(leaf)\n"
            "print(\"complete\")\n"
        ),
        "leaf.py": "# first line\n\n\"\"\"module\ndocumentation\"\"\"\nfrom __future__ import annotations\nprint(\"leaf\")\n",
    },
    "reload_source_plugin": {
        "main.py": (
            "from importlib import *\n"
            "import importlib\n"
            "assert importlib.__all__ == ['__import__', 'import_module', 'invalidate_caches', 'reload']\n"
            "assert '__import__' in globals() and 'sys' not in globals()\n"
            "from sys import modules\n"
            "name = '.'.join(('plugins', 'report'))\n"
            "plugin = import_module(name)\n"
            "render = plugin.render\n"
            "assert render('first') == ('first', 1)\n"
            "assert import_module('.report', 'plugins') is plugin\n"
            "assert reload(plugin) is plugin is modules[name]\n"
            "assert plugin.render is not render\n"
            "print(plugin.render('new'), render('saved'))\n"
        ),
        "plugins/__init__.py": "",
        "plugins/report.py": (
            "generation = globals().get('generation', 0) + 1\n"
            "def render(label):\n    return label, generation\n"
        ),
    },
    "boxed_unittest_lazy_import": {
        "main.py": (
            "import sys\nimport unittest\n\n"
            "print(unittest.TestCase.__name__, 'unittest.async_case' in sys.modules)\n"
        ),
        # A multiprocessing cleanup helper behind unittest's lazy getter imports test.support.
        "test/__init__.py": "",
        "test/support/__init__.py": "from importlib.machinery import ModuleSpec\n",
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
    "frozen_shadow": {"main.py": "import __hello__\n", "__hello__.py": HELPER},
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
            "import email\n"
            "from email import message_from_bytes\n"
            "from email._header_value_parser import get_addr_spec, get_angle_addr as angle\n"
            "from email import message_from_binary_file\n"
            "from io import BytesIO\n"
            "import email._header_value_parser as parser\n"
            "assert angle is parser.get_angle_addr\n"
            "assert email.message_from_bytes is message_from_bytes\n"
            "print(email.message_from_bytes(b'Subject: hello\\r\\n\\r\\nbody')['Subject'])\n"
            "print(message_from_bytes(b'\\r\\nraw\\xff').get_payload(decode=True))\n"
            "for parse, value in ((get_addr_spec, 'user@example.org SIZE=32'), (angle, '<>')):\n"
            "    address, rest = parse(value)\n"
            "    print(address.addr_spec, rest)\n"
            "with BytesIO(b'Subject: hi\\r\\n folded\\r\\n\\r\\nraw\\xff') as fp:\n"
            "    msg = message_from_binary_file(fp)\n"
            "    print(msg['Subject'], msg.get_payload(decode=True), fp.read())\n"
        ),
    },
    "zip_source_spec": {
        "main.py": (
            "import sys\nimport zipimport\nfrom importlib import import_module, invalidate_caches\n"
            "from importlib.util import find_spec\nfrom importlib.resources import files\nfrom witness import seen\n"
            "from pkgutil import get_importer, iter_modules\n"
            "sys.path.insert(0, __file__.rsplit('/', 1)[0] + '/zip-provider')\n"
            "def refresh():\n    assert sys.exception() is None\n    refresh_finder()\n"
            "for name in ('stored', 'deflated'):\n"
            "    sys.path.insert(0, __file__.rsplit('/', 1)[0] + '/' + name + '.zip')\n"
            "    assert find_spec(name) is not None and name not in seen\n"
            "    package = import_module(name)\n    spec = find_spec(name)\n"
            "    assert package is import_module(name) and package.__spec__ is spec\n"
            "    assert spec.loader is package.__loader__ and type(spec.loader) is zipimport.zipimporter\n"
            "    assert spec.submodule_search_locations is package.__path__\n"
            "    roots = [sys.path[0], sys.path[0]]\n"
            "    assert list(iter_modules(roots)) == [(get_importer(roots[0]), name, True)]\n"
            "    assert list(iter_modules(package.__path__, name + '.')) == [\n"
            "        (get_importer(package.__path__[0]), name + '.leaf', False)]\n"
            "    from importlib.readers import ZipReader\n"
            "    reader = spec.loader.get_resource_reader(name)\n"
            "    assert isinstance(reader, ZipReader) and reader.prefix == name + '/'\n"
            "    leaf = import_module(name + '.leaf')\n"
            "    assert leaf.word == 'café'\n"
            "    adjacent = leaf.__loader__.get_resource_reader(leaf.__name__)\n"
            "    assert adjacent.archive == reader.archive and adjacent.prefix == reader.prefix\n"
            "    root = files(leaf)\n"
            "    assert 'message.txt' in [member.name for member in root.iterdir()]\n"
            "    if name == 'stored':\n"
            "        assert root.joinpath('message.txt').read_bytes() == b'hello'\n"
            "        try:\n            root.joinpath('missing.txt').read_bytes()\n"
            "        except FileNotFoundError:\n            pass\n"
            "        else:\n            raise AssertionError('missing ZIP resource opened')\n"
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
            "from importlib.abc import FileLoader, SourceLoader\n"
            "from importlib._abc import Loader\n"
            "from importlib.util import Loader as UtilLoader\n"
            "def check():\n"
            "    assert abc is protocols and FileLoader is abc.FileLoader\n"
            "    assert SourceLoader is protocols.SourceLoader\n"
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
    "lazy_loader": {
        "main.py": (FIXTURES.parent / "importlib_lazy_loader.py").read_text(),
        "price_events.py": "calls = []\n",
        "price_plugin.py": (
            "import price_events\nprice_events.calls.append(__name__)\ncurrency = 'USD'\n"
            "def quote(value): return value + 2\n"
        ),
    },
    "builtin_flag_release": {
        "main.py": (FIXTURES.parent / "builtin_flag_release.py").read_text(),
    },
    "frozen_importer": {
        "main.py": (FIXTURES.parent / "frozen_importer.py").read_text(),
        "frozen_price_plugin.py": "def quote(n): return n * 7\n",
    },
    "builtin_method_creation": {
        "main.py": (FIXTURES.parent / "builtin_method_creation.py").read_text(),
    },
    "builtin_importer": {
        "main.py": (FIXTURES.parent / "builtin_importer.py").read_text(),
        "price_plugin.py": "executions = globals().get('executions', 0) + 1\ndef quote(n): return n * 7\n",
    },
    "source_loader_external_code": {
        "main.py": (FIXTURES.parent / "source_loader_external_code.py").read_text(),
        "local_body.py": "value = 41\n",
    },
    "source_loader_namespace_directory": {
        "main.py": (
            "import os\n"
            "from types import ModuleType\n"
            "from importlib.machinery import SourceFileLoader\n"
            "import ns.child\n"
            "try:\n"
            "    SourceFileLoader('ns', ns.__path__[0]).get_code('ns')\n"
            "except OSError:\n"
            "    pass\n"
            "else:\n"
            "    raise AssertionError('file loader returned code for a namespace directory')\n"
            "path = ns.__path__[0] + '/late.py'\n"
            "with open(path, 'wb'):\n"
            "    pass\n"
            "try:\n"
            "    loader = SourceFileLoader('late', path)\n"
            "    assert loader.get_code('late').co_filename == path\n"
            "    loader.exec_module(ModuleType('late'))\n"
            "finally:\n"
            "    os.unlink(path)\n"
        ),
        "ns/child.py": "",
    },
    "bundled_source_loader_data": {
        "main.py": (FIXTURES.parent / "bundled_source_loader_data.py").read_text(),
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
            "import importlib.util\n"
            "from importlib.util import find_spec\nfrom importlib import import_module\n"
            "first = find_spec('orchard.leaf')\nsecond = find_spec('.leaf', 'orchard')\n"
            "print(first == second, first is second, first.parent, first.loader.is_package(first.name))\n"
            "leaf = import_module('orchard.leaf')\npackage = import_module('orchard')\n"
            "print(leaf.marker, find_spec(first.name) is leaf.__spec__, leaf.__spec__.loader is leaf.__loader__)\n"
            "print(package.__path__ is package.__spec__.submodule_search_locations, "
            "leaf.__file__ == first.origin, find_spec('orchard.missing') is None)\n"
            "leaf.__spec__ = 'app spec'\nprint(find_spec('.leaf', 'orchard'))\n"
            "blocked = RuntimeError('parent import')\n"
            "def deny_parent(name, /, *, fromlist):\n"
            "    assert name == 'orchard' and fromlist == ['__path__']\n"
            "    raise blocked\n"
            "importlib.util.__import__ = deny_parent\n"
            "assert find_spec('.leaf', 'orchard') is leaf.__spec__\n"
            "try:\n"
            "    find_spec('.missing', 'orchard')\n"
            "except RuntimeError as error:\n"
            "    assert error is blocked\n"
            "else:\n"
            "    raise AssertionError('find_spec skipped the parent import')\n"
            "del importlib.util.__import__\n"
        ),
        "orchard/__init__.py": "print('parent init')\n",
        "orchard/leaf.py": "print('leaf init')\nmarker = __name__\n",
    },
    "source_method_factories": {
        "factory.py": (
            "def label(n: int):\n"
            "    print('factory', n)\n"
            "    def apply(value):\n"
            "        print('apply', n)\n"
            "        return n\n"
            "    return apply\n"
        ),
        "main.py": (
            "import factory as labels\n"
            "class Result:\n"
            "    @labels.label(2)\n"
            "    @labels.label(3)\n"
            "    def value(self): pass\n"
            "print(Result().value)\n"
        ),
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
    "child_export_shadows_module": {
        "package/__init__.py": "from .token import number as token\n",
        "package/token.py": "number = 42\n",
        "main.py": (
            "import package\nfrom package import token\nimport package.token as alias\n"
            "from package.token import number as direct\n"
            "print(package.token)\nprint(token)\nprint(alias)\nprint(direct)\n"
        ),
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
    "module_expression_before_import": {
        "main.py": "from provider import seen\nprint(seen)\n",
        "state.py": "values: list[int] = []\n",
        "provider.py": "from state import values\nvalues.append(7)\nfrom observer import seen\n",
        "observer.py": "from state import values\nseen = values[0]\n",
    },
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


def check_saved_package_files(args, temporary, build_flags):
    # A FILE operand identifies the entry even when the build/run cwd offers a namesake.
    module = "saved_entry.tools.driver"
    entry_path = "saved_entry/tools/driver.py"
    shadow = temporary / "shadow"
    write_files(shadow, {
        "saved_entry/__init__.py": "", "saved_entry/tools/__init__.py": "",
        entry_path: 'print("cwd entry")\n',
        "saved_entry/tools/leaf.py": 'value = "cwd sibling"\n',
    })
    shadowed = run([args.python, "-E", "-B", "-m", module], shadow)
    require_success(shadowed)
    assert shadowed.stdout == "cwd entry\n"
    for regular, level in ((False, args.opt_levels[0]), (True, args.opt_levels[-1])):
        root = temporary / ("regular-entry" if regular else "namespace-entry")
        files = {
            entry_path: (
                "import sys\nfrom .leaf import value\nfrom ..outer import marker\n"
                "assert __name__ == '__main__' and globals() is sys.modules['__main__'].__dict__\n"
                "assert __spec__.name == 'saved_entry.tools.driver' and __spec__.name not in sys.modules\n"
                "assert __package__ == __spec__.parent == 'saved_entry.tools'\n"
                "assert __loader__ is __spec__.loader and __spec__.has_location\n"
                "assert __loader__.get_code(__spec__.name).co_filename == __file__ == __spec__.origin\n"
                "assert sys.argv[0] == sys.argv[1]\n"
                "print(value, marker)\n"
                "if len(sys.argv) == 3:\n"
                "    try: __import__(sys.argv[2])\n"
                "    except ModuleNotFoundError as e: print('missing', e.name)\n"
                "    else: print('present')\n"
            ),
            "saved_entry/tools/leaf.py": 'value = "nested"\n',
            "saved_entry/outer.py": 'marker = "outer"\n',
            "saved_entry/tools/unreferenced.py": 'value = 41\n',
        }
        if regular:
            files["sitecustomize.py"] = (
                "import sys\nfrom types import ModuleType\n"
                "held = ModuleType('__main__')\nheld.token = object()\nsys.modules['__main__'] = held\n"
            )
            files["saved_entry/__init__.py"] = (
                "import sys\nfrom sitecustomize import held\n"
                "assert held is sys.modules['__main__'] and not hasattr(held, '__file__')\n"
                "held.parent_token = held.token\nprint('parent')\n"
            )
            files["saved_entry/tools/__init__.py"] = 'print("child")\n'
            files[entry_path] += (
                "import builtins\nfrom sitecustomize import held\n"
                "assert globals() is held.__dict__ and parent_token is held.token\n"
                "assert __builtins__ is builtins.__dict__\n"
            )
        write_files(root, files)
        entry = str(root / entry_path)
        baseline = run([args.python, "-E", "-B", "-m", module, entry], root)
        require_success(baseline)
        assert baseline.stdout == ("parent\nchild\n" if regular else "") + "nested outer\n"
        product = root.parent / (root.name + "-application")
        command = [str(args.compiler.resolve()), "build", entry, "--runtime", str(args.runtime.resolve()),
                   "--opt-level", str(level), "--clang", args.clang, "-o", str(product), *build_flags]
        # Infer the enclosing regular package. Namespace portions need their declared root.
        if not regular:
            command += ["--module-root", str(root)]
        if args.sanitize:
            command.append("--sanitize")
        require_success(run(command, shadow, timeout=300))
        executable = str(product.rename(shadow / product.name))
        root.rename(root.with_suffix(".withdrawn"))
        unused = "saved_entry.tools.unreferenced"
        saved = run([executable, executable, unused], shadow)
        require_success(saved)
        assert saved.stdout == baseline.stdout + "missing " + unused + "\n" and saved.stderr == "", saved


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

        check_saved_package_files(args, temporary, build_flags)
        cached = temporary / "package-cache"
        write_files(cached, {
            "pkg/__init__.py": "import sys\nsys.modules['opcode'] = sys\n",
            "pkg/main.py": "import __main__\nimport opcode\nprint(opcode.__name__)\n",
            "__main__.py": "import opcode\n",
            "opcode.py": "def take(x: int) -> int: return x\ndef text(): return 'wrong'\nprint(take(text()))\n",
        })
        require_success(run([compiler, "check", str(cached / "pkg/main.py"), "--module-root", str(cached)], temporary))
        checks += 3

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
                # The boxed ZIP reader uses the reference interpreter's zipfile.Path.
                location = run([args.python, "-I", "-c", "import zipfile; print(zipfile.__file__)"], case)
                require_success(location)
                source = Path(location.stdout.strip())
                provider = case / "zip-provider"
                provider.mkdir()
                if source.name == "__init__.py":
                    shutil.copytree(source.parent, provider / "zipfile", ignore=shutil.ignore_patterns("__pycache__"))
                else:
                    shutil.copyfile(source, provider / "zipfile.py")
                for pkg, method in [("stored", ZIP_STORED), ("deflated", ZIP_DEFLATED)]:
                    with ZipFile(case / f"{pkg}.zip", "w", compression=method) as archive:
                        archive.writestr(f"{pkg}/__init__.py", "# coding: (ordinary comment)\nfrom witness import seen\nseen.append(__name__)\n")
                        archive.writestr(f"{pkg}/leaf.py", b"# coding: latin-1\r\nword = 'caf\xe9'\r\n")
                        archive.writestr(f"{pkg}/message.txt", b"hello")
            baseline = run([args.python, "-E", "-m", "main"], case)
            require_success(baseline)
            if name in {"codeop_module", "codeop_named_helper"}:
                # Run these modules through the registered imports below.
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
                # Optimizing these bundled-module integration cases can take several minutes.
                slow_optimized = name in {
                    "boxed_unittest_lazy_import", "builtin_importer", "bundled_source_loader_data", "frozen_importer",
                    "builtin_method_creation", "email_address_parsers", "filesystem_resources", "fractions_indirect",
                    "function_type_for_selected_callback", "lzma_formats_and_error",
                    "optional_stdlib_probe_in_dependency", "platform_iot_indirect", "record_warnings",
                    "sdk_import_after_provider_hint", "source_loader_external_code", "source_loader_namespace_directory",
                    "warn_named_helper",
                    "zip_source_spec",
                    "builtin_flag_release",
                }
                timeout = 300 if slow_optimized and level > 0 else build_timeout
                require_success(run(command, temporary, timeout=timeout))
                arguments = ["41"] if name == "source_loader_external_code" else []
                result = run([str(executable), *arguments], temporary)
                require_success(result)
                assert result.stdout == baseline.stdout, (name, level, result.stdout, baseline.stdout)
                if name == "entry_module_attribute_guard":
                    denied = run([str(executable), "deny"], temporary)
                    expected = run([args.python, "-E", "-m", "main", "deny"], case)
                    assert denied.returncode == expected.returncode == 1
                    assert denied.stdout == expected.stdout == "feature loaded\n"
                    assert denied.stderr.splitlines()[-1] == expected.stderr.splitlines()[-1]
                executable.unlink()
                checks += 1

        (temporary / "members.gz").write_bytes(gzip.compress(b"first", mtime=0) + gzip.compress(b"second", mtime=0))
        for level in args.opt_levels:
            project = temporary / f"provider-imports-O{level}"
            project.mkdir()
            for name in ("codeop_module", "codeop_named_helper"):
                (project / f"{name}.py").write_bytes((temporary / name / "main.py").read_bytes())
            (project / "helper.py").write_bytes((temporary / "codeop_named_helper" / "helper.py").read_bytes())
            interactive = "boxed_interactive_interpreter"
            type_aliases = "boxed_type_alias_values"
            for fixture in (interactive, type_aliases):
                shutil.copyfile(FIXTURES.parent / f"{fixture}.py", project / f"{fixture}.py")
            entry = project / "main.py"
            # Select a known provider at execution time. All Python sources exist before the build.
            providers = ("opcode", "_opcode_metadata", "code", "codeop", "codeop_module",
                         "codeop_named_helper", interactive, type_aliases, "gzip")
            entry.write_text(
                "import importlib, sys\n"
                "def run():\n"
                + "".join(f"    {'if' if i == 0 else 'elif'} sys.argv[1] == {name!r}:\n"
                          f"        module = importlib.import_module({name!r})\n"
                          for i, name in enumerate(providers))
                + "    else:\n        importlib.import_module('no_such_console_guard_module')\n"
                "    if sys.argv[2] == 'flags':\n"
                "        print(module.CommandCompiler().compiler.flags)\n"
                "    elif sys.argv[2] == 'lookup':\n"
                "        print(module.opmap['POP_TOP'])\n"
                "    elif sys.argv[2] == 'current':\n"
                "        module.current()\n"
                "    elif sys.argv[2] == 'write':\n"
                "        import zlib\n"
                "        member = module.compress(memoryview(b'payload'), compresslevel=1, mtime=42)\n"
                "        assert member[4:10] == bytes((42, 0, 0, 0, 4, 255))\n"
                "        assert zlib.decompress(member, wbits=31) == b'payload'\n"
                "        with module.open('written.gz', 'wb') as stream:\n"
                "            print(stream.write(b'payload'))\n"
                "            assert stream.seekable() and stream.tell() == 7\n"
                "            stream.flush()\n"
                "    elif sys.argv[2] == 'read':\n"
                "        with module.open('members.gz', 'rb') as stream:\n"
                "            print(stream.read1(65536), stream.read(1), stream.tell(), stream.read())\n"
                "        print(stream.closed)\n"
                "        from io import BytesIO\n"
                "        with open('members.gz', 'rb') as raw:\n"
                "            with module.GzipFile(fileobj=raw) as reader:\n"
                "                assert reader.peek(1).startswith(b'f') and reader.tell() == 0\n"
                "                assert reader.read(3) == b'fir' and reader.rewind() is None\n"
                "                assert reader.tell() == 0 and reader.read(5) == b'first'\n"
                "            raw.seek(0)\n"
                "            source = BytesIO(b'prefix' + raw.read())\n"
                "        source.seek(6)\n"
                "        with module.GzipFile(filename='metadata.gz', fileobj=source) as stream:\n"
                "            assert stream.read1(65536) == b'first'\n"
                "            assert stream.seek(1, 1) == 6 and stream.read() == b'econd'\n"
                "        assert not source.closed\n"
                "        stamped = BytesIO(module.compress(b'one', mtime=0) + module.compress(b'two', mtime=0xfedcba98))\n"
                "        with module.GzipFile(fileobj=stamped) as stream:\n"
                "            assert stream.mtime is None and stream.read(0) == b'' and stream.mtime is None\n"
                "            assert stream.read1() == b'one' and stream.mtime == 0\n"
                "            assert stream.read() == b'two' and stream.mtime == 0xfedcba98\n"
                "        assert stream.mtime == 0xfedcba98 and not stamped.closed\n"
                "        class ObservedSource:\n"
                "            def __init__(self, data):\n"
                "                self.raw, self.times = BytesIO(data), []\n"
                "            def read(self, size):\n"
                "                self.times.append(self.reader.mtime)\n"
                "                return self.raw.read(size)\n"
                "        observed = ObservedSource(module.compress(b'payload', mtime=23))\n"
                "        with module.GzipFile(fileobj=observed, mode='rb') as observed.reader:\n"
                "            assert observed.reader.read() == b'payload'\n"
                "        assert observed.times[0] is None and 23 in observed.times\n"
                "        class Reader(module.GzipFile):\n"
                "            def read(self, *args):\n"
                "                raise AssertionError('readline called read')\n"
                "        with Reader('members.gz', 'rb') as stream:\n"
                "            print(stream.readline(0), stream.readline(3), stream.readline(8), stream.readline(1))\n"
                "            try:\n"
                "                stream.readline(-(1 << 100))\n"
                "            except OverflowError:\n"
                "                pass\n"
                "            else:\n"
                "                raise AssertionError('unrepresentable line size accepted')\n"
                "\nrun()\n"
            )
            executable = temporary / f"provider-imports-{level}"
            command = [compiler, "build", str(entry), "-o", str(executable),
                       "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                       *build_flags, "--no-cache"]
            if args.sanitize:
                command.append("--sanitize")
            require_success(run(command, temporary, timeout=build_timeout))
            # The saved image must initialize the application modules from its registrations.
            for source in project.glob("*.py"):
                source.unlink()
            for module, operation, expected in [
                ("opcode", "lookup", "31\n"), ("_opcode_metadata", "lookup", "31\n"),
                ("code", "flags", "16896\n"), ("codeop", "flags", "16896\n"),
                ("codeop_module", "import", ""),
                ("codeop_named_helper", "import", "16896\n"),
                (interactive, "current", ""),
                (type_aliases, "current", "checked lazy aliases\n"),
                ("gzip", "write", "7\n"),
                ("gzip", "read", "b'first' b's' 6 b'econd'\nTrue\nb'' b'fir' b'stsecond' b''\n"),
            ]:
                result = run([str(executable), module, operation], temporary)
                require_success(result)
                assert result.stdout == expected, (module, level, result.stdout)
                checks += 1

            assert gzip.decompress((temporary / "written.gz").read_bytes()) == b"payload"

            absent = "no_such_console_guard_module"
            result = run([str(executable), absent, "flags"], temporary)
            assert result.returncode == 1 and f"ModuleNotFoundError: No module named '{absent}'" in result.stderr, result
            checks += 1

        # These are build capabilities: CPython's flags depend on its build; Imre emits native code.
        for level in args.opt_levels:
            for fixture, output in [
                ("opcode_flags", "0\n0\nno bytecode specialization\n"),
                ("opcode_stack_effect", "-1\n"),
                ("opcode_module", "opcode module OK\n"),
                ("opcode_operations", "opcode operations OK\n"),
            ]:
                executable = temporary / f"{fixture}-{level}"
                command = [compiler, "build", str(root / f"{fixture}.py"), "-o", str(executable),
                           "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                           *build_flags]
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

        if sys.platform in ("linux", "darwin"):
            constants = ["SO_ERROR"]
            if sys.platform == "linux":
                constants.extend(["SOCK_NONBLOCK", "SOCK_CLOEXEC", "TCP_QUICKACK"])
            for module in ("socket", "_socket"):
                case = temporary / f"{module}_constants_keep_typed_calls"
                write_files(case, {"main.py": (
                    f"from {module} import {', '.join(constants)}\n"
                    "def typed(n: int) -> int: return n\n"
                    f"def mask() -> int: return {' | '.join(constants)}\n"
                    "def dormant() -> int:\n"
                    "    return typed('wrong')\n"
                    "print(mask())\n"
                )})
                result = run([compiler, "check", str(case / "main.py")], temporary)
                assert result.returncode != 0 and "main.py:5:" in result.stderr, result
                assert "expected int, got str" in result.stderr or "invalid-argument-type" in result.stderr, result
                checks += 1

        for name, (files, expected) in REJECTED.items():
            case = temporary / name
            write_files(case, files)
            result = run([compiler, "check", str(case / "main.py")], temporary)
            assert result.returncode != 0 and expected in result.stderr, (name, result.stdout, result.stderr)
            checks += 1

        # A provider can return to the application's check without acquiring its signature policy.
        app = temporary / "provider-owner"
        provider = temporary / "provider-search"
        call = (
            "def take(value: int) -> int: return value\n"
            "def text(): return 'wrong'\n"
            "print(take(text()))\n"
        )
        for entry, helper, diagnostic in (
            ("import helper\n" + call + "import opcode\n", "def marker(): pass\n", "main.py:4:12: Imre: expected int, got str"),
            ("import helper\nimport opcode\n", call, None),
        ):
            write_files(app, {"main.py": entry})
            write_files(provider, {"helper.py": helper})
            result = run([compiler, "check", str(app / "main.py"), "--extension-path", str(provider)], temporary)
            if diagnostic is None:
                require_success(result)
            else:
                assert result.returncode != 0 and diagnostic in result.stderr, result.stderr
            checks += 1

        write_files(app, {"main.py": "import _imre_decimal_ratio\n" + call + "import opcode\n"})
        result = run([compiler, "check", str(app / "main.py")], temporary)
        assert result.returncode != 0 and "main.py:4:12: Imre: expected int, got str" in result.stderr, result.stderr
        checks += 1

        # Check even uncalled functions when a different function requires boxed operations.
        mock_setup = "from unittest.mock import patch\nclass Owner:\n    value = 0\n"
        invalid = "def uncalled() -> int:\n    return f('wrong')\n"
        for setup, consumer in (
            (mock_setup, "def helper() -> None:\n    patch.object(Owner, 'value', 1)\n"),
            (mock_setup, "def helper() -> object:\n    return patch.object(Owner, 'value', 1)\n"),
            ("from math import atan2\n", "def helper() -> float:\n    return atan2(1.0, -1.0)\n"),
            ("from math import exp\n", "def helper() -> float:\n    return exp(1.0)\n"),
            ("from math import frexp\n", "def helper() -> float:\n    return frexp(6.0)[0]\n"),
            ("from math import fsum\n", "def helper() -> float:\n    return fsum([1.0, 2.0])\n"),
            ("from math import isfinite\n", "def helper() -> bool:\n    return isfinite(1.0)\n"),
            ("from math import sin\n", "def helper() -> float:\n    return sin(1.0)\n"),
            ("from math import sqrt\n", "def helper() -> float:\n    return sqrt(4.0)\n"),
            ("from math import tan\n", "def helper() -> float:\n    return tan(1.0)\n"),
            ("from random import uniform\n", "def helper() -> float:\n    return uniform(-1.0, 1.0)\n"),
            ("from random import expovariate\n", "def helper() -> float:\n    return expovariate(0.5)\n"),
            ("from sys import float_repr_style\n", "def helper() -> str:\n    return float_repr_style\n"),
            ("from socket import htons\n", "def helper() -> int:\n    return htons(4660)\n"),
            ("from _socket import htons\n", "def helper() -> int:\n    return htons(4660)\n"),
        ):
            for before, after in ((invalid, consumer), (consumer, invalid)):
                source = temporary / "boxed_call_order.py"
                source.write_text(setup + HELPER + before + after)
                result = run([compiler, "check", str(source)], temporary)
                assert result.returncode != 0 and (
                    "expected int, got str" in result.stderr or "invalid-argument-type" in result.stderr
                ), (source.read_text(), result.stderr)
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
            # A consistent return type still admits the imported AST operation.
            corrected = function.replace("return flag\n", "return int(flag)\n").replace("return 'one'\n", "return 0\n")
            (case / "helper.py").write_text("import ast\ntree = ast.parse('1')\n" + corrected + "choose(True)\n")
            require_success(run(command, temporary))
            checks += 3

        caller = "def gzip_caller() -> bytes: return compress(b'payload')\n"
        typed = "def typed(n: int) -> int: return n\n"
        unused = "def unused() -> int: return typed('wrong')\n"
        for name, definitions, error_line in (
            ("caller_first", caller + typed + unused, 4),
            ("caller_last", unused + typed + caller, 2),
        ):
            case = temporary / f"gzip_compress_{name}"
            write_files(case, {"main.py": "from gzip import compress\n" + definitions + "print(gzip_caller())\n"})
            result = run([compiler, "check", str(case / "main.py")], temporary)
            assert result.returncode != 0 and f"main.py:{error_line}:" in result.stderr, result
            assert "expected int, got str" in result.stderr or "invalid-argument-type" in result.stderr, result
            checks += 1

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
