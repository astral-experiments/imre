# The builtins namespace in the pinned suites

CPython commit: `5607950ef232dad16d75c0cf53101d9649d89115`. The 97
`test.test_sys` instances come from a single test file. The four whole-module
checks (the import and runner entry points with full and reachable checking)
stopped at that file's `import builtins` on line 1 at Imre `76d74f9`.
With native access to `builtins._`, all four progress to the unimplemented
`import codecs` on line 2; none has reached unittest discovery natively.

The only direct builtins operations in this suite are the read, deletion and
absence check on lines 51, 53 and 59 of `DisplayHookTest.test_original_displayhook`.
The [process fixture](display_values.py) compares native stdout, stderr and exit
status against pinned Python at both optimization levels. It checks absence and
errors, repr and text, list mutation and indexing, object retention across
replacement and deletion, numeric equality, and the difference between an absent
binding and a binding to None. The retained value shares its runtime representation
with native exception fields, including a tuple captured from SystemExit.code.
It does not add an upstream test pass.

Other direct namespace uses in the 12 pinned module suites, including their
test packages and child-program strings:

| Consumer | Upstream use | Behavior required |
| --- | --- | --- |
| collections | `test_ordered_dict.py:789` | A class holds the builtins module and the actual dict class; the selected inherited tests use the fixture. Static class/module resolution is [#567](https://github.com/astral-sh/imre/pull/567). |
| functools | `test_functools.py:1773–1779` | Save, replace and restore the live `len` binding with an LRU-wrapped callable. |
| logging | `test_logging.py:5249–5267` | Remove `builtins.open` in a child. Its FileHandler must still have the previously captured open callable at finalization. |
| argparse | `test_argparse.py:2069` | Patch `builtins.open` and observe which parser calls use it. |
| unittest | `test_unittest/test_discovery.py:878,911`, `test_unittest/testmock/testmock.py:263` | Patch the actual import function for discovery and patch open for mock. |

A qualified native `_` lookup reads its current value. A from-import executes
its lookup once at the import statement and retains the object. A missing
from-import raises ImportError with the original message and the module and
export names; a qualified read raises AttributeError. The
other callable bindings above also remain unsupported. Mapping their names to
the default native operations would bypass changes to the namespace.

The two later codecs operations in this suite are `codecs.lookup(fs_encoding)`
at line 975 and `codecs.charmap_build(encodings.iso8859_3.decoding_table)` at
line 1650. Host codecs in an assertion or fixture do not establish native codec
support.


The complete original `DisplayHookTest.test_original_displayhook` runs at both
optimization levels with CPython's fixture, body and assertions. Its ten
operations reach one persistent Imre process. Newly written native stdout
bytes are forwarded to the original captured stream. The child itself owns
`builtins._`, saves and deletes its `sys.stdout`, raises the missing-stream
exception, and restores the native handle. This is host-driven native
operation coverage.
