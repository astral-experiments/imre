from collections.abc import Sequence
from difflib import ndiff

def delta(left: Sequence[str], right: Sequence[str]) -> list[str]:
    return list(ndiff(left, right))

print(repr(delta([], [])))
print(repr(delta(["same", "same", "tail"], ["same", "tail"])))
print(repr(delta(["a", "a", "b"], ["a", "a", "c"])))
print(repr(delta(["a", "z", "x", "z"], ["z", "a", "z", "x"])))
print(repr(delta(["before", "old 1", "old 2", "after"], ["before", "new", "after"])))
print(repr(delta(["spoon\n"], ["spawn\n"])))
print(repr(delta(["\tcafé 🦀!\n"], ["\tcafÉ 🦀!\n"])))
print(repr(delta(["abcdefghijk"], ["abcXdefghijk"])))
print(repr(delta(["abcXdefghijk"], ["abcdefghijk"])))
print(repr(delta(["abcde", "same", "abcde"], ["abcdé", "same", "abcdé"])))
popular = ["same"] * 210
print(repr(delta(["start", *popular], [*popular, "end"])))
