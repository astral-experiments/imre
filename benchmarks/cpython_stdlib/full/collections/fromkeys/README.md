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

The fixture supplies a constructed set to `fromkeys`. The [nested set control](probes/set_range.py)
retains `dict.fromkeys(set(range(6)), 0)` from the original case. On the first
[Linux run](https://github.com/astral-sh/imre/actions/runs/40181575398), all four
compiler modes rejected the inner `range` before the mapping operation ran.
The complete original method stopped earlier, on its first dictionary comparison
(`test_dict.py:336`): native `TestCase.assertEqual` required an integer operand.

Stock [`TextWrapper` in CPython 3.14.5](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/textwrap.py#L66)
uses `dict.fromkeys(map(ord, _whitespace), ord(' '))`. The distinct keys are
9, 10, 11, 12, 13 and 32, each with value 32. The combined integration must
run the original class initializer to test both dependencies together.

## Linux receipts

The [complete original method](runs/a99beb43679f/fromkeys-original.json) is
[blocked once at each level](evidence-linux.json). Its first dictionary assertion
is tracked with the [unittest comparison owner](https://github.com/astral-sh/imre/pull/710#issuecomment-6396217239).

The [combined run](https://github.com/astral-sh/imre/actions/runs/40182742723)
used [these immutable compiler and runtime inputs](runs/a99beb43679f/source.json):
public ord/map `501d59a` and mapping `49a44de`. Its
[stock TextWrapper receipts](runs/a99beb43679f/textwrapper.json) distinguish
three scopes. The exact expression returned the six ordered pairs in all four
modes. Both the original class assignment and the full stock module stopped at
the computed class attribute, before the expression was evaluated. Reproduce
the reference with CPython 3.14.5 using `textwrapper.py --result reference.json`;
pass `--imre`, `--runtime`, and `--clang` for O0 and O3 with ordinary and
reachable-only compilation.

The [14 independent mapping controls](runs/a99beb43679f/controls.json) include
the seven local class and iterator definitions from the original method. All
seven stop at the declaration; none demonstrated native subclass or custom
iterator dispatch. Five other controls matched at all four settings: exact dict
inputs, a shared mutable value, a non-iterable, and the bound Counter and
defaultdict operations. On this tree the event fixture stopped at `list.clear`
and the nested set control at `set(range(6))`. Later PR changes give the two
event checks separate lists; [#910](https://github.com/astral-sh/imre/pull/910)
supplies the set producer. These controls are outside both the single selected
`test_dict` occurrence and the 659 occurrences in collections.
