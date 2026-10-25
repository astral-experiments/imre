# Collections startup dependencies

## The array import in test_collections

The unchanged module imports `array` on line 3. Its only use, across the
vendored collections tests and their inherited helpers, is
[`TestCollectionABCs.test_MutableSequence`](upstream/Lib/test/test_collections.py#L1975):
`self.assertIsSubclass(array.array, MutableSequence)`. The test never
constructs an array or invokes array methods.

The required behavior is an importable module whose `array` attribute denotes
the array type, and virtual subclass registration with
`collections.abc.MutableSequence`. CPython's
[`arraymodule.c`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Modules/arraymodule.c#L3301)
registers the type when the module initializes. On the pinned interpreter its
base is `object`, its virtual ABCs include `MutableSequence` and `Sequence`,
and it is not a `MutableMapping`.

The same routine checks negative and positive class and instance cases for
tuple, str, bytes, list, bytearray and deque, then calls the upstream helper to
validate the abstract length, subscription, assignment, deletion and insertion
methods. Resolving the import alone does not pass that routine.

On [the tested array tree](array/7ae08a4f0d74/source.json), the separate
[registration fixture](array/7ae08a4f0d74/array-four-modes.json) matched all 532
CPython observations at O0 and O3 with both ordinary and reachable-only compilation.
The complete `test_MutableSequence` was still blocked importing `collections.deque`
in its fixture at [O0](array/7ae08a4f0d74/abc-O0.json) and [O3](array/7ae08a4f0d74/abc-O3.json).
All 24 original ABC methods remain blocked on this tree. The
[full per-occurrence reports](array/7ae08a4f0d74/evidence-linux.json) preserve
659 at each level: 7 native bodies with host fixtures, 13 host-driven native
operations, 4 without a collections operation, 3 reference skips and 632 blocked.

After [deque #561](https://github.com/astral-sh/imre/pull/561) landed, the
[array and deque merge tree](array/60fafd7a2c54/source.json) still matched
[all 532 registration observations](array/60fafd7a2c54/array-four-modes.json).
With its imports resolved, the original `test_MutableSequence` now reaches
[`self.assertNotIsInstance(sample(), MutableSequence)` at line 1970](upstream/Lib/test/test_collections.py#L1970)
at [O0](array/60fafd7a2c54/abc-O0.json) and [O3](array/60fafd7a2c54/abc-O3.json).
All 24 methods still compile-stop on this tree. Four [unittest controls](array/60fafd7a2c54/unittest-assertions.json)
use only builtin classes and isolate `assertIsInstance`, `assertNotIsInstance`,
`assertIsSubclass` and `assertNotIsSubclass`. CPython distinguishes each positive
case from its deliberately false one. Imre reports “unknown class method” on all
four in each compiler mode. The [unittest handoff](https://github.com/astral-sh/imre/pull/934#issuecomment-6397068344)
tracks this shared dependency.

The [four original collections startup checks](array/7ae08a4f0d74/startup.json)
now reach [`import pickle` at line 9](upstream/Lib/test/test_collections.py#L9)
and execute zero tests. In the primary suite four original methods use pickle:
[`TestChainMap.test_basics`](upstream/Lib/test/test_collections.py#L127) checks a round trip with
distinct component maps; [`TestNamedTuple.test_pickle`](upstream/Lib/test/test_collections.py#L568)
looks up `loads` and `dumps` dynamically and checks both the fields and the bytes;
[`test_field_descriptor`](upstream/Lib/test/test_collections.py#L687) restores field descriptors
into a different tuple class; [`TestCounter.test_copying`](upstream/Lib/test/test_collections.py#L2228)
checks distinct, equal Counter results. The pinned interpreter's highest protocol is 5.
The namedtuple instance test also uses protocol -1.

The primary module has just one doctest call:
[`tests.addTest(doctest.DocTestSuite(collections))`](upstream/Lib/test/test_collections.py#L2416),
which contributes 15 occurrences. The deque companion calls
[`doctest.DocTestSuite()`](upstream/Lib/test/test_deque.py#L1033) without an
argument; the caller's `__test__['libreftest']` contributes one more.
Both are included in the 659-occurrence denominator.

## Mapping as the base of a real standard-library class

[CPython's `_colorize.ThemeSection`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/_colorize.py#L115) is the argparse dependency reached after the adjacent f-string depth fix in [#576](https://github.com/astral-sh/imre/pull/576). The class declaration uses `Mapping[str, str]` as a base. On the pinned interpreter its runtime base is `collections.abc.Mapping`; the parameterized alias survives in `__orig_bases__`.

`ThemeSection` implements subscription, length, and key iteration. Its frozen `Argparse` dataclass subclass has 13 fields in declaration order. On an actual instance, `isinstance(section, Mapping)` is true, present-key `get` calls its subscription method, missing-key `get` returns the default, and membership and key/item/value views work from the three primitives. The concrete dataclass supplies its own equality; Mapping mixins must respect subclass overrides. Importing the name or erasing the base would not establish this behavior. The class also uses dataclass metadata, a bound dictionary getter and dynamic attribute lookup; Mapping alone does not prove that the complete argparse module can run.

The original [`TestCollectionABCs.test_Mapping`](upstream/Lib/test/test_collections.py#L1842) also checks dict instance/subclass membership, abstract-method enforcement, comparison against an unrelated object and the disabled reverse protocol of a custom Mapping subclass. [`test_MutableMapping_subclass`](upstream/Lib/test/test_collections.py#L1864) checks the ABC identities of the views returned by `UserDict`. These remain original collections IDs in the baseline. The reference observations above add no native passes.

## Top-level copy and the container protocols it calls

The original tests below call `copy.copy` or `copy.deepcopy`, sometimes through
a helper or as a stored callable. The copy module owns dispatch, reconstruction
and memo handling. Collections owns its types' state, copy methods and
reconstruction protocols. In particular, copying the backing mappings of a
ChainMap cannot be substituted with copying each mapping at every depth.

| Upstream cases (relative to `test.`) | Observable behavior |
| --- | --- |
| [`test_collections.TestUserObjects.test_list_copy` and `test_dict_copy`](upstream/Lib/test/test_collections.py#L43) | Both call the same original helper. The wrapper gets independent `.data`; the shallow copy's other instance attribute points to the original list. |
| [`test_collections.TestChainMap.test_basics`](upstream/Lib/test/test_collections.py#L118) | The first map is distinct in a shallow copy and every parent is shared. Deep copy produces a distinct but equal map at *every* level. The same routine separately checks pickle and repr. |
| [`test_collections.TestNamedTuple.test_copy`](upstream/Lib/test/test_collections.py#L579) | Both copy functions are taken as first-class values; the reconstructed module-level `TestNT` compares equal and retains `_fields`. |
| [`test_collections.TestCounter.test_copying`](upstream/Lib/test/test_collections.py#L2216) | Both copies are distinct, equal Counters. This routine also exercises `.copy()`, all pickle protocols, repr/eval, update, and construction from another Counter. |
| [`test_defaultdict.TestDefaultDict.test_shallow_copy` and `test_deep_copy`](upstream/Lib/test/test_defaultdict.py#L98) | Preserve the current factory, first an upstream function, then `list`; deep copy clones the nested list as well as the mapping. |
| [`test_deque.TestBasic.test_copy` and `test_deepcopy`](upstream/Lib/test/test_deque.py#L675) | A distinct deque shares its inner list on shallow copy and separates it on deep copy. The shallow test separately checks the method's bound and unbounded maximum lengths and element identity. |
| [`test_ordered_dict.*.test_copying`](upstream/Lib/test/test_ordered_dict.py#L297) | Six variants: C and pure Python, each as the base class, subclass and slotted subclass. Copies retain the exact type, ordered items and attributes. Shallow copies share the two mutable attributes; deep copies separate them. Missing attributes stay missing. The routine also checks pickle and repr. |
| [`test_free_threading.test_collections.TestDeque.test_copy_race`](upstream/Lib/test/test_free_threading/test_collections.py#L10) | The imported `copy` callable must work during concurrent appends and pops. This single ID is import-skipped by the pinned GIL-enabled reference; it stays in the denominator. |

These are 16 occurrences, including the one reference skip. A native container
method alone does not establish that an original call through the copy module
dispatches natively.
