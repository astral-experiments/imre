# dict.fromkeys

The focused dependency case is [CPython's complete `DictTest.test_fromkeys`](upstream/Lib/test/test_dict.py#L335).
It is one of 120 occurrences discovered in `test.test_dict` on pinned Linux
CPython 3.14.5, separate from the [659 collections occurrences](../README.md).
The complete file and [license](upstream/LICENSE) match [the upstream Git blobs](UPSTREAM.json).

The original method tests class and instance calls, strings, tuples, an empty
list, a generator, dict/set/list inputs, and a non-iterable. Its dict subclasses
inherit the constructor, return a UserDict or existing dictionary from
`__new__`, raise in `__init__` or `__setitem__`, and initialize one thousand
existing entries. The iterator raises its own exception. A native plain-dict
probe cannot certify those subclass paths.

The separate [differential fixture](../../../../../tests/fixtures/containers/dict_fromkeys.py)
also checks mutable value identity, argument evaluation before pulling keys,
and bound Counter and defaultdict behavior. Probe output does not add any
upstream occurrence.

Stock [`TextWrapper` in CPython 3.14.5](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/textwrap.py#L66)
uses `dict.fromkeys(map(ord, _whitespace), ord(' '))`. The distinct keys are
9, 10, 11, 12, 13 and 32, each with value 32. The combined integration must
run the original class initializer to test both dependencies together.
