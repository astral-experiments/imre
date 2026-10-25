# Native equality in JSON_checker round trips

CPython's `test_pass1`, `test_pass2`, and `test_pass3` each define one
`test_parse` method. Each method parses its original document, serializes the
result, parses it again, and calls `self.assertEqual` on the two parsed values.
Both upstream backend fixtures invoke the same native Imre implementation.

The [measured report](roundtrips-equality-linux.json) records both optimization
levels and the six concrete fixture IDs. With a native assertion that compares
its operands directly, all six unchanged bodies pass. All three independent
controls with an unequal nested leaf raise native `AssertionError`. The
test body, JSON operations, and assertion run in Imre; CPython runs the original
fixture lifecycle.

With the inherited native `unittest.TestCase.assertEqual`, all six executions
still fail because the assertion requires integer operands. All three documents
serialize to the same bytes as the pinned CPython reference before the assertion.
These are alternate observations for the same six occurrences in the 216-case
package, not additional suite passes.

To repeat this comparison, run the [reproducer from #660](https://github.com/astral-sh/imre/blob/9540b2d86b4d95c5204824c396b4ae85854e4c13/benchmarks/cpython_stdlib/full/json/roundtrips.py)
with the pinned CPython binary, the candidate Imre compiler and runtime, and its
commit as `--imre-revision`.
