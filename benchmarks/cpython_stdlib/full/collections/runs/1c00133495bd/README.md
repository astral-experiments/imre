# Sequence predicates in the original MutableSequence test

The [O0 and O3 reports](evidence-linux.json) retain all 659 pinned
occurrences on the [tested source](source.json). At each level 11 complete
native bodies used host fixtures, 13 host bodies exercised native operations,
one case mixed native and host operations, four had no collections operation,
three were skipped by upstream, and 627 were blocked. No complete occurrence
ran entirely natively.

The original `TestCollectionABCs.test_MutableSequence` runs under CPython's
fixtures and unittest assertions. Imre evaluates all eight `issubclass`
calls, including `issubclass(array.array, MutableSequence)` at upstream line
1975, and five `isinstance` calls. CPython checks the bytearray instance,
builds the method dictionaries, and constructs the six classes in the
original abstract-method helper. The [O0](sequence-mixed-O0.json) and
[O3](sequence-mixed-O3.json) receipts list every native operand and answer.
Inverting either the array subclass answer or the list instance answer fails
the original assertion.

The attempt to compile the whole method still stops at `assertNotIsInstance`
on line 1970. The [O0](abc-O0.json) and [O3](abc-O3.json) results retain
the separate native-body attempts for all 24 collection ABC methods.
The [56 supplemental comparisons](sequence-four-modes.json) cover
class metadata, registered and unrelated classes, immutable sequences,
typed deques, union and optional operands, and receiver evaluation order.
They add no occurrences.
