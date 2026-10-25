# Annotation dependencies of test_functools

This inventory uses the same CPython 3.14.5 commit as [UPSTREAM.json](UPSTREAM.json).
The complete upstream functools suite has **325 IDs**. Startup checks and
standalone annotation probes do not run any of those test bodies.

[The release-wide client inventory](annotation-clients.json) records the Git
blob, import lines, and qualified API reads in all 17 direct importers under
`Lib/`. [The generator](annotation_clients.py) verifies each scanned source
containing `annotationlib` against the pinned Git tree. Imported names and
qualified reads are separate: for example, `inspect` reexports
`get_annotations`, and `typing` dispatches through a lazy module proxy.
The seven library clients are `_collections_abc`, `dataclasses`, `functools`,
`inspect`, `pydoc`, `typing`, and `unittest.mock`; the remaining ten files
are test support and tests. The sole qualified read in `test.support` is
`annotationlib.ForwardRef` at line 3171, followed by four ordinary instance
attribute comparisons at lines 3174–3177.

After native `_opcode` landed in [#800](https://github.com/astral-sh/imre/pull/800),
the untouched `unittest`, `pathlib`, and `inspect` package and child entry points
all reached `Lib/test/support/__init__.py:19`, which imports `types` (24 checks).
`test_functools` still stopped at its line 3 `import builtins` (four checks).
`test_annotationlib` stopped at a starred tuple display on line 186 (four checks).
The [32 startup diagnostics](annotation-startup-after-opcode-linux.json) retain
the import or runner entry, reachability setting, compiler hash, and original
error for each check. The measured tree is identical to canonical `a1d9a53`.

| Upstream source | Git blob |
| --- | --- |
| Lib/annotationlib.py | 5c9a0812646f8149eb4afa4da1fdaafeb5422bf3 |
| Lib/functools.py | df4660eef3fe820d2396b69ec4541f6e66c8be4b |
| Lib/test/support/__init__.py | 6635ec3474e12ec6a2f684a1a93b6ec08df49a37 |
| Lib/test/test_annotationlib.py | 5087c3ca425f1fc8d44c051c9bb52e7a7f955100 |

| Caller | What it observes |
| --- | --- |
| TestLRUC and TestLRUPy, test_get_annotations | Fresh dictionaries for the original and decorated function containing the actual builtin int and str class objects. |
| The same classes, test_get_annotations_with_forwardref | FORWARDREF preserves the known int and represents the unresolved return. The reference's owner is the specific object passed (original or wrapper). VALUE raises NameError. |
| test.support.EqualToForwardRef.__eq__ | Checks isinstance against annotationlib.ForwardRef; reads __forward_arg__, __forward_module__, __forward_is_class__, and __owner__. The helper is test setup; producing the reference and its owner is product behavior. |
| TestUpdateWrapper and TestWraps, test_update_wrapper_annotations | The wrapper's __annotate__ is the original callable. Reading __annotations__ raises until an enclosing cell is assigned, then observes its new value. |
| TestSingleDispatch, test_annotations, test_forward_reference and test_unresolved_forward_reference | Decorator registration looks at the first annotated argument. An unresolved later annotation is allowed; an unresolved dispatch argument raises TypeError. functools loads Format and ForwardRef lazily through typing.get_type_hints. |
| TestImportTime.test_lazy_import | Checks that importing functools itself does not import annotationlib (among other modules). Running CPython's sys.modules probe is not a native import result. |

Annotationlib also exports call_annotate_function, call_evaluate_function,
get_annotate_from_class_namespace, annotations_to_string and type_repr. Only
get_annotations and Format are imported directly by the functools test file.
Exports used indirectly by setup or user code need separate implementation and
verification; resolving the imports alone exercises none of them.

The native declaration reader builds fresh dictionaries for known functions,
classes and modules, including builtin class values, quoted annotations and
symbolic STRING expressions. It neither invokes CPython nor uses a host
annotation dictionary. The two fixtures in tests/fixtures/callables exercise
this behavior at -O0 and -O3. Unresolved references, arbitrary namespace
evaluation and wrapper metadata are separate from this declaration reader.
The four standard Format constants expose their integer values and declared
names. Constructing an IntEnum dynamically and reflecting on the enum class
are outside this implementation. [run_annotationlib.py](run_annotationlib.py)
verifies the pinned source and license and runs the complete upstream
TestFormat.test_enum at both optimization levels. [The results](annotationlib-linux.json)
record a native method body and assertions with CPython fixture hooks; this is
a dependency test, not one of the 325 functools IDs.

Native `ForwardRef(arg, *, module, owner, is_argument, is_class)` instances
retain their constructor fields, including the original callable owner. The
native constructor, attribute reads, equality, nominal `isinstance`, and
representation of supported fields are exercised by
`tests/fixtures/callables/forward_refs.py`. These direct operations do not
establish that deferred annotations have been evaluated on a wrapped function.
Hashing, union operators, evaluation, and inspection of private evaluator cells
require further support.

[Three complete upstream ForwardRef routines](annotation-forwardrefs-linux.json)
exercise the non-string constructor argument and invalid class targets of
`isinstance` and `issubclass` at O0 and O3. The bodies, errors and
`assertRaises` assertions ran in Imre; the original module/class/instance
fixtures ran on CPython. Reproduce with `run_annotationlib.py --forward-refs`
and the same toolchain options as for `TestFormat`. This run used the
ForwardRef implementation at `69f7e15` and the unittest assertion context at
`87e5d20` (local integration `20c7de7`). The report reproduced byte for byte at
`178166df` with the merged unittest assertion context (`1f4a7ed1`).
These are supplemental annotation
tests, separate from the 325 functools and four free-threaded occurrences.

The native declaration evaluator can now represent an unknown bare name on a
known function. With `Format.VALUE` it raises `NameError` when annotations
are read. With `Format.FORWARDREF` it returns a fresh annotation dictionary
that can contain both a builtin class and an owned `ForwardRef`.
`annotations_deferred.py` compares the known class, all public properties of
the unresolved return, the original owner across local and module reads,
independent local function identities, and later dictionary mutation with
CPython. These operations are native; the fixture is outside the functools
denominator. A cache wrapper's metadata, user-supplied evaluation namespaces
and arbitrary annotation expressions need further support.

Using the untouched import and unittest entry points from the coordinator's
76d74f9 startup run (both default and reachable-only), the first diagnosis
moves from test_functools.py:2 (annotationlib) to line 3 (builtins). The two
LRU variants of test_lru_reentrancy_with_len save, replace and restore
builtins.len. These **four failed startup attempts** establish the next import;
they do not change the number of complete native test bodies.

The [six package startup checks](annotation-package-startup-linux.json) use
the coordinator's combined guard and ImportError head, `03977539`, plus the
two annotation changes, and the complete pinned test package. In both
check modes `import test.support` advances from line 6 to line 9,
`import inspect`. The actual uses in that module are `_static_getmro`
(2849), `_get_dunder_dict_of_class` (2851), and `_sentinel` (2860/2864).
The direct import and runner for `test.test_functools` still stop at
`import builtins` (line 3) in each mode. These checks execute no test body.

[#586](https://github.com/astral-sh/imre/pull/586) landed at
`fccfcc7c974fb7d9248ff44f06f8fbd04f508cef`. On that commit the native
annotation fixtures and unchanged `TestFormat.test_enum` reproduced their
O0 and O3 results. The [coordinator's paired startup checks](annotation-shared-startup-linux.json)
compare that commit with the same source plus native `inspect`. For
`unittest`, `pathlib`, and `inspect`, both the package and a child module
move from `test/support/__init__.py:9` (`inspect`) to line 11 (`_opcode`).
All 24 import/runner checks per revision stopped before executing a test.
The four `test_functools` startup checks still stop at `builtins` on line 3.
