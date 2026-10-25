# Original deque basics with native list reversal

At O0 and O3 the 659 occurrences include 12 native bodies under CPython
fixtures, 13 host-driven native operations, four without a collections
operation, three reference skips and 627 blocked. None have fully native
fixtures. The parent had 11 native bodies and 628 blocked.

The unchanged `TestBasic.test_basics` grows a deque to 600 items and checks
both ends after popping 250 items from each. The body, assertions and
operations run in Imre; module, class and case hooks run in CPython. With
only `imre_list_reverse` disabled by the linker, its assertion finds 399
where it expected 150. Regenerate `deque-basics-O*.json` with the module's
`deque_basics.py`. The 31 differential observations in the four-mode
receipt do not add suite occurrences.

The inherited list method has different receivers. The UserList fixture
still stops on the missing export. The separate builtin-list case stops
on the class attribute `type2test = list`. A typed fixture method delegating
to the builtin constructor reaches the unchanged `assertRaises(TypeError,
u.reverse, 42)` at line 402. Its builtin receiver is unsupported by the
shared unittest callback; see [the report to its owner](https://github.com/astral-sh/imre/pull/947#issuecomment-6399494677).
Neither inherited case is promoted.
