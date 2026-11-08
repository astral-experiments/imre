# EnvironTests on Linux

The table records Imre `a6edd3ae3b12ece7ed81aab01bb07cbd2208c4b6`. Its boxed builds used the explicit runtime-import option. [Linux results by original case ID](linux-results.json)

| Run | O0 | O3 |
| --- | ---: | ---: |
| Stock CPython 3.14.5 (full module, one reference run) | 32/32 | same reference |
| Ordinary Imre (retained class) | 0 executed | 0 executed |
| Boxed Imre (retained class) | 32/32 | 32/32 |

Stock and both boxed runs had zero skips. The boxed runner discovered the same 32 IDs in the same order as stock. Each case restored both Python mapping views and the environment inherited by a fresh process. Both intentionally incorrect `test_items` runs reported an assertion failure and passed their cleanup checks.

That revision’s ordinary compiler stopped before discovery at `mapping_tests.py:31`, where `BasicTestMappingProtocol.__init__` calls `unittest.TestCase.__init__(self, *args, **kw)`. The typed setup and teardown probes supply a constructor and exercise real environment mutation with native children. Only complete-class runs enter the totals.

The entry passes the real `test.test_os.EnvironTests` class to `unittest.defaultTestLoader.loadTestsFromTestCase`. Every original case, including methods inherited from `mapping_tests.BasicTestMappingProtocol`, runs alone in a fresh executable process. Its original setup, teardown, cleanup and module teardown run inside that process. The parent checks that its own bytes environment never changes; each test process records the NUL-delimited output of `/usr/bin/env -0` before and after the unittest run. Each child also compares independent dictionary copies of both Python mapping views before and after the original. Only the comparison status is emitted. A case can earn a verified pass only if the text view, byte view and process environment are restored, the parent environment is unchanged, all 32 IDs match the pinned reference in order and the original reports success. A missing or failed audit leaves the original unittest outcome visible and the case unverified. The child receives a small, known environment so results do not expose caller environment values.

Use a pristine CPython source directory at commit `5607950ef232dad16d75c0cf53101d9649d89115` and its built CPython 3.14.5 interpreter. Run the supervisor using CPython 3.11 or newer. The staged package uses hard links to the original test helpers, falling back to copies across filesystems. This keeps ordinary-mode imports inside the first-party module root. The retained-class layout includes the complete upstream support package and mapping_tests.py. A full layout stages the entire test package; put the work directory on the same filesystem as the CPython checkout to minimize disk use. The supervisor verifies the Git blobs for the affected test sources and records the native toolchain hashes and commands.

    python3 replay.py --cpython-source "$cpython" --python "$cpython/python" \
        --engine stock --layout full --work /dev/shm/environ-stock

    python3 replay.py --cpython-source "$cpython" --python "$cpython/python" \
        --engine ordinary --layout retained-class --level 0 --imre "$imre" \
        --runtime "$runtime" --clang "$clang" --case test_items \
        --case test_environb --work /dev/shm/environ-first-ordinary-O0

Use fresh work directories for each run. Repeat with `--level 3`. Omit `--case` to run all 32. Under `--layout full` the entire original test_os.py is staged. Under `--layout retained-class` only the complete, unmodified class, its imports, and original `tearDownModule` survive the projection; original line numbers are preserved. Results from those layouts are separate.

A separate run with `--wrong` changes only the expectation in the original `test_items` assertion to the actual expected value plus `!`. The supervisor returns success for this negative control only if that single original reports FAIL, the wrong value appears in its assertion, and all environment checks succeed. In all other runs the supervisor returns success only when every selected case has a verified pass. The result file in each work directory retains raw build and case stdout/stderr, skips, failures, errors and exit statuses. A failure to import the module or complete discovery yields a diagnostic instead of case credit.
