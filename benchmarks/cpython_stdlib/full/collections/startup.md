# The array import in test_collections

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
