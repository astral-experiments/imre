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

## Mapping as the base of a real standard-library class

[CPython's `_colorize.ThemeSection`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/_colorize.py#L115) is the argparse dependency reached after the adjacent f-string depth fix in [#576](https://github.com/astral-sh/imre/pull/576). The class declaration uses `Mapping[str, str]` as a base. On the pinned interpreter its runtime base is `collections.abc.Mapping`; the parameterized alias survives in `__orig_bases__`.

`ThemeSection` implements subscription, length, and key iteration. Its frozen `Argparse` dataclass subclass has 13 fields in declaration order. On an actual instance, `isinstance(section, Mapping)` is true, present-key `get` calls its subscription method, missing-key `get` returns the default, and membership and key/item/value views work from the three primitives. The concrete dataclass supplies its own equality; Mapping mixins must respect subclass overrides. Importing the name or erasing the base would not establish this behavior. The class also uses dataclass metadata, a bound dictionary getter and dynamic attribute lookup; Mapping alone does not prove that the complete argparse module can run.

The original [`TestCollectionABCs.test_Mapping`](upstream/Lib/test/test_collections.py#L1842) also checks dict instance/subclass membership, abstract-method enforcement, comparison against an unrelated object and the disabled reverse protocol of a custom Mapping subclass. [`test_MutableMapping_subclass`](upstream/Lib/test/test_collections.py#L1864) checks the ABC identities of the views returned by `UserDict`. These remain original collections IDs in the baseline. The reference observations above add no native passes.
