# CPython 3.14.5: types

The files under `upstream/` are unmodified CPython Git blobs. `UPSTREAM.json`
pins the complete test module, its Python and C implementation sources and its
license. On a test-enabled Linux release build of the pinned interpreter,
`reference-linux.json` records every discovered test: 128 passed and one was
skipped because the interpreter did not have `sys.gettotalrefcount`.

## What runs natively

At both `-O0` and `-O3`, `run.py` runs the original bodies of three
`SimpleNamespaceTests` methods, byte for byte:

| Method | Operation | Assertions |
| --- | --- | --- |
| `test_attrget` | Read an attribute; catch a missing attribute | Native `unittest.TestCase.assertEqual` and `assertRaises` |
| `test_equal` | Compare different namespace instances before and after writes | Native adapter calling equality and inequality on the original operands |
| `test_unbound` | Access the attribute dictionaries of temporary instances | Native adapter comparing map lengths and every key and value |

CPython supplies discovery and the original fixture hooks. Imre supplies the
method body, assertion object and product operations. `native-linux.json`
retains the 129-case denominator at each optimization level: three native
bodies with host fixture, 125 reference-only passes and one upstream skip.
Compiling the whole suite entry is a separate check; it does not run these
methods.

The extra process comparison in `namespaces.py` includes the constructor
expression used by CPython's `test.support.run_in_subinterp_with_config`,
input evaluation order, shallow copies, live dictionary access, mutation after
the namespace is destroyed, ordering, identity, missing attributes and display.
For the native evidence run, use a test-enabled CPython 3.14.5:

```sh
python benchmarks/cpython_stdlib/full/types/run.py \
  --cpython-source /path/to/cpython \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /path/to/clang --result /tmp/types-native.json
```

The native constructor supports a typed string-keyed dictionary, explicit
keywords, or a single keyword spread. Attribute values share one supported
type. An empty constructor starts with an integer-valued dictionary. Mixed
layouts, user-defined value hooks, floating-point values (including nested
values), arbitrary pair iterables and subclassing are outside this contract.
The rejection controls include a shared NaN: dictionary equality must
recognize the same object even though NaN does not compare equal to itself.

## Complete suite startup

`startup-linux.json` compares two compilers on the same 21 unchanged suite
entries, using both a plain import and the standard unittest runner, each with
and without `--reachable-only`. The before tree includes native `stat`,
static `inspect` lookup and the two native `_opcode` specialization flags.
The after tree adds only the native namespace implementation. Both sets of 84
checks stop during compilation; **zero tests ran**.

Eight entries get past `test/support/__init__.py:19: import types` in all
four modes (32 changed diagnostics). In the regular compiler mode all eight
then stop at `import warnings`. Reachable compilation instead exposes a
class-body restriction in `test.support`, or a dependency on `weakref`,
`errno`, `annotationlib.ForwardRef` or `_imp`, depending on the suite.
The other 52 checks keep their original diagnostic, including
`test.test_types` itself: the starred tuple display at line 873 is not yet
supported. Each entry's exact diagnostics and the tested compiler hashes are
in the receipt.
