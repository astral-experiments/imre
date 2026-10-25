# The builtins namespace in the pinned suites

CPython commit: `5607950ef232dad16d75c0cf53101d9649d89115`. The 97
`test.test_sys` instances come from a single test file. The four whole-module
checks (the import and runner entry points with full and reachable checking)
stopped at that file's `import builtins` on line 1 at Imre `76d74f9`.
Native access to `builtins._` moved all four to `import codecs` on line 2. With
native codec lookup all four now reach `import gc` on line 3; none has reached unittest discovery natively.

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

A qualified native `_` lookup reads its current value. A from-import would
snapshot it at import time; the compiler currently rejects that form. The
other callable bindings above also remain unsupported. Mapping their names to
the default native operations would bypass changes to the namespace.

The two later codecs operations in this suite are `codecs.lookup(fs_encoding)`
at line 975 and `codecs.charmap_build(encodings.iso8859_3.decoding_table)` at
line 1650. The original filesystem method and helper both execute in Imre, including lookup; `charmap_build` remains unsupported.
The [native codec fixture](codec_lookup.py) compares aliases and cache identity, canonical
names, exact encoded/decoded results and consumed lengths, errors, and the five fixed
UTF byte-order marks with pinned Python. The stateless registry supports ASCII, Latin-1,
UTF-8 and UTF-8 with signature. Search-function registration, stream codecs, other
encodings, mutable CodecInfo attributes, and surrogate-preserving text need further work.

The pinned os and logging suites use different parts of this registry. In
`test.test_os.DeviceEncodingTests.test_device_encoding` (`test_os.py:3554`),
the original assertion calls `codecs.lookup` with the encoding of file
descriptor 0. With redirected stdin the original decorator skips the case before
lookup. With a pseudo-terminal it runs, passing the host OS's `UTF-8` encoding
to an Imre child. That child looks up the codec and returns its canonical name
(`utf-8`) and truth. CPython runs the unchanged method, assertion and fixtures.
The [Linux observations](codec-device-linux.json) at O0/O3 include a stock run
for each configuration. They add no sys occurrence and preserve os's canonical
redirected skip. Reproduce them with the pinned Python on Linux:

```sh
"$CPYTHON" -B benchmarks/cpython_stdlib/full/sys/codec_device.py \
  --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
  --cpython-source "$CPYTHON_SOURCE" --result codec-device-linux.json
```

In `test_logging.py:2306`, `test_encoding_cyrillic_unicode`
obtains `codecs.getwriter('cp1251')`, sets its class encoding, and logs into a
wrapped `io.BytesIO`. A stateless UTF-8 or Latin-1 codec cannot implement that
writer. These consumer occurrences stay with their respective suite reports.
