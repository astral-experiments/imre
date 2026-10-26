# Non-callable defaultdict factories

The [O0 and O3 reports](evidence-linux.json) cover all 659 pinned CPython
3.14.5 occurrences on the [tested compiler and runtime](source.json).
At each level, 11 complete native bodies used host fixtures, 14 host bodies
exercised native operations, four cases had no collections operation, three
were skipped by upstream, and 627 were blocked. No complete fixture, body,
assertions and operation all ran natively.

In the unchanged `TestDefaultDict.test_callable_arg`, CPython runs the
original method, fixtures and assertion. An Imre executable constructs the
supplied dictionary and calls `collections.defaultdict`. The native
constructor raises `TypeError: first argument must be callable or None`.
The original assertion fails when the child returns normally and when it
raises `ValueError`. The [O0](factory-host-O0.json) and
[O3](factory-host-O3.json) receipts retain the three child outcomes.

Compiling that whole method still stops at its empty dictionary argument at
upstream line 141. The [O0](defaultdict-methods-O0.json) and
[O3](defaultdict-methods-O3.json) attempts preserve the first blocker for
each of the 13 original defaultdict methods. The [16 supplemental outputs
in four compiler modes](factory-four-modes.json) cover argument evaluation
order, earlier exceptions, a non-callable instance and a valid list factory;
they do not add suite occurrences.
