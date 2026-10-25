# Reinitializing deques on Linux

The unchanged CPython 3.14.5 `test.test_deque.TestBasic.test_basics` now
compiles its call to `d.__init__(range(200))` at line 34. Its next diagnostic
at both O0 and O3 is the unsupported list method `right.reverse()` at line
47. The body did not run. Both full reports retain this occurrence as blocked.

The versioned report covers all 659 collections occurrences at each level:
11 native bodies with host fixtures, 13 host-driven native operations, four
bodies without a collections operation, three reference skips and 628 blocked.
None ran with fully native fixtures. The class reports retain all complete
`TestBasic` attempts; `test-basics.json` records an independent attempt at
just the original method through the shared suite runner.

`differential.json` is separate from the denominator: 63 lines for deque
reinitialization and 65 for existing deque behavior agree with CPython in
both whole-program and reachable-only builds at O0 and O3. These include
argument evaluation order, stable receiver identity, `maxlen` validation
before mutation, and partial results after a generator raises. Use the
source and binary digests in `source.json` to identify this run.
