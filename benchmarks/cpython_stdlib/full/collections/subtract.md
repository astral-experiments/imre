# Counter subtraction

The [O0](subtract-host-O0.json) and [O3](subtract-host-O3.json) reports run the
entire unchanged `TestCounter.test_subtract`. A CPython unittest fixture drives
24 calls into native Imre: 11 constructors, seven bound subtractions, three
equality comparisons, two ordered item reads and one unbound call. The host
retains source statements so each observation can replay the preceding native
state. It supplies the original assertions; it neither calculates nor stores
the counts. The three expected TypeErrors originate in Imre.

Run `counter_host.py` with test-enabled CPython 3.14.5 and `--imre`,
`--runtime`, `--clang`, `--opt-level`, and `--result`. Separately, the
five-prompt `collections.Counter.subtract` doctest runs as a complete native
body with host output checking at both levels. These are two distinct IDs out
of the 659 in the reference.
