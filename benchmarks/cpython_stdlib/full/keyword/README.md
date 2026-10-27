# keyword

Imre bundles [CPython 3.14.5's generated keyword module](upstream/Lib/keyword.py). The public lists are mutable. Its two predicates retain the original frozen sets even when a caller edits or replaces the lists. Jinja2 imports `iskeyword` under the name `is_python_keyword` and calls it when writing keyword arguments in a generated function call.

All 11 [original tests](upstream/Lib/test/test_keyword.py) pass on CPython 3.14.5. The [ordinary](evidence-linux.json) and [reachable-only](reachable-linux.json) reports each measure every original method at O0 and O3.

| Outcome at both optimization levels | Methods |
| --- | ---: |
| Native body, assertions, keyword operations and default unittest fixture | 6 |
| Native body, assertions and keyword operations with the original CPython lifecycle and a native assertion/cleanup fixture | 3 |
| Native execution failed | 2 |

The three host-assisted cases pass `None` to the saved predicate or replace one of the exported lists. The cleanup fixture restores the original list object. Both failures exercise Python source execution. `test_all_keywords_fail_to_be_used_as_names` encounters a missing boxed `SyntaxError`. `test_all_soft_keywords_can_be_used_as_names` reaches a one-argument `exec` that requires explicit globals on this runtime.

Run the [suite runner](run.py) with the test-enabled pinned CPython, `--imre`, `--runtime`, `--clang` and `--result`. Pass `--reachable-only` for the second mode. It verifies the upstream blobs, tries every original method with native unittest and records the actual fixture for each fallback. The native fallback runs negative assertion and cleanup controls before it can contribute a passing observation.
