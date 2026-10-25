# Annotation dependencies of test_functools

This inventory uses the same CPython 3.14.5 commit as [UPSTREAM.json](UPSTREAM.json).
The complete upstream functools suite has **325 IDs**. Startup checks and
standalone annotation probes do not run any of those test bodies.

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

Using the untouched import and unittest entry points from the coordinator's
76d74f9 startup run (both default and reachable-only), the first diagnosis
moves from test_functools.py:2 (annotationlib) to line 3 (builtins). The two
LRU variants of test_lru_reentrancy_with_len save, replace and restore
builtins.len. These **four failed startup attempts** establish the next import;
they do not change the number of complete native test bodies.
