# Native deque reversal

The [per-occurrence report](evidence-linux.json) covers all 659 pinned CPython cases at O0 and O3 on the [tested compiler and runtime](source.json). Each level has 12 native bodies and assertions under host fixtures, 14 with operations driven or checked by the host, four with no collections operation, three reference skips and 626 blocked. No entire occurrence ran natively.

The [O0](deque_reverse_host-O0.json) and [O3](deque_reverse_host-O3.json) runs of the original randomized method constructed 500 native deques, reversed them 1,000 times, inspected their contents and lengths 1,000 times each, and invoked one native arity error. CPython provided the random inputs and ran the body, assertions and fixture hooks. Replacing native reverse with a no-op failed the unchanged assertion at line 417 at both levels.

The direct body stops on its random import. A [separate diagnostic](native-body-boundary.json) supplied exact CPython random values and reached the unsupported assertion on lists of floats at line 417. This diagnostic does not earn a native pass.

The 33 [deque reversal](deque_reverse-four-modes.json), 31 [list reversal](list_reverse-four-modes.json) and 63 [deque reinitialization](deque_reinitialize-four-modes.json) observations matched CPython in all four compilation modes. The [deque basics case](deque_basics-O0.json) still ran its body and assertions natively in the host fixture.

Five O0 Counter doctests initially failed to link because the devbox ran out of disk space. The [first attempt](doctests-first-O0.json) and [repeat of all 16](doctests-retry-O0.json) retain the failures and retry. Those five were the only changed outcomes, with identical binary hashes.
