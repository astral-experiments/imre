# Standard library test omissions

The compatibility table keeps every original test in its upstream denominator. Each accepted omission identifies one occurrence and the behavior Imre can reasonably leave unsupported. Paired passes and observed skips keep their original meanings.

To add a decision, cite the assertion in the pinned CPython source, explain its impact on Python programs, and identify the public behavior that remains in scope. A failure, timeout or skip is evidence for review. The decision is made per original test.

## Collections

All eight originals below passed on stock CPython 3.14.5 and skipped at both Imre optimization levels in the unchanged 295-case OrderedDict file. The companion tests named in the table passed at both levels. [CPython source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py) · [Imre revision](https://github.com/astral-experiments/imre/commit/b5af3ae9c19865f07fc3910132390a7ff1f32828)

### Accepted omissions

| Original test | Occurrence | Decision | Related passing test in the same class |
| --- | ---: | --- | --- |
| `test.test_ordered_dict.CPythonOrderedDictSubclassTests.test_ordered_dict_items_result_gc` | 1 | The subclass inherits a test of the C iterator's recycled tuple. Subclass item iteration preserves the key and value without prescribing when the collector tracks the result. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `test_views` |
| `test.test_ordered_dict.CPythonOrderedDictSubclassTests.test_sizeof_exact` | 1 | This subclass inherits a byte-count formula for CPython's C object, table entries, nodes and iterators. A subclass can report a different footprint and still implement the sizing protocol. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L887-L917) | `test_sizeof` |
| `test.test_ordered_dict.CPythonOrderedDictTests.test_ordered_dict_items_result_gc` | 1 | The assertion inspects whether the collector tracks an item tuple after CPython recycles it. An iterator can return the same key and value using a fresh tuple. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `test_views` |
| `test.test_ordered_dict.CPythonOrderedDictTests.test_sizeof_exact` | 1 | The expected bytes come from CPython's C structs, pointer widths, hash-table allocation and node sizes. Imre can report the size of its own representation. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L887-L917) | `test_sizeof` |
| `test.test_ordered_dict.PurePythonOrderedDictSubclassTests.test_ordered_dict_items_result_gc` | 1 | The pure-Python subclass checks the collector's tracking bit after its first item. A different tuple lifetime strategy can satisfy the subclass iteration contract. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `test_views` |
| `test.test_ordered_dict.PurePythonOrderedDictTests.test_ordered_dict_items_result_gc` | 1 | The pure-Python provider runs the same tuple-tracking assertion. The required item values are observable regardless of the collector's tracking policy. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `test_views` |

### Reviewed cases still in scope

| Original test | Occurrence | Decision | Related passing test in the same class |
| --- | ---: | --- | --- |
| `test.test_ordered_dict.CPythonOrderedDictSubclassTests.test_weakref_list_is_not_traversed` | 1 | A subclass, a self-referential list and a weak reference go out of scope before collection. A crash would affect ordinary subclass users. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L954-L971) | `test_reference_loop` |
| `test.test_ordered_dict.CPythonOrderedDictTests.test_weakref_list_is_not_traversed` | 1 | The body creates a reference cycle containing a weak reference and calls the public collector. A crash in that sequence is observable in an ordinary program. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L954-L971) | `test_reference_loop` |
