# Counter.elements

The [original five-prompt doctest](upstream/Lib/collections/__init__.py#L645)
sorts the elements of a string Counter and multiplies the elements of an integer
Counter. The latter part imports `math` after the first two prompts have run.
The whole routine remains blocked: the compiler rejects this late import,
and the native standard library also needs `math.prod`. Its expected output
and its occurrence in the 659-test reference are unchanged.

The separate [compiler fixture](../../../../tests/fixtures/containers/counter_elements.py)
matched CPython 3.14.5 on all 31 output lines, exit status, and stderr at O0 and
O3. It checks native iteration over live mapping entries, signed count overflow,
mutation during a repetition, lifetime of record keys after deletion, and
bound and unbound argument errors. A nested call with an invalid argument
still fails at the nested expression. The host compares program output;
this fixture adds no upstream ID.

Verification commit `b74f7a5c20d5a53c46ccf8780da7d82526540758` ran all 16
original collections/deque docstrings and all 22 original `TestCounter`
methods at both levels. Eleven docstring bodies ran with host output checks;
five were blocked. Seven unittest bodies ran under their host fixtures and
15 failed to compile. These counts include the arithmetic already on main.
