# EnvironTests on Linux

The entry passes the real `test.test_os.EnvironTests` class to `unittest.defaultTestLoader.loadTestsFromTestCase`. Every original case, including methods inherited from `mapping_tests.BasicTestMappingProtocol`, runs alone in a fresh executable process. Its original setup, teardown, cleanup and module teardown run inside that process. The parent checks that its own bytes environment never changes; each test process records the NUL-delimited output of `/usr/bin/env -0` before and after the unittest run. A case can earn a verified pass only if both checks pass, all 32 IDs match the pinned reference in order and the original reports success. The child receives a small, known environment so results do not expose caller environment values.

Use a pristine CPython source directory at commit `5607950ef232dad16d75c0cf53101d9649d89115` and its built CPython 3.14.5 interpreter. Run the supervisor using CPython 3.11 or newer. The staged package uses hard links to the original test helpers, falling back to copies across filesystems. This keeps ordinary-mode imports inside the first-party module root. The retained-class layout includes the complete upstream support package and mapping_tests.py. A full layout stages the entire test package; put the work directory on the same filesystem as the CPython checkout to minimize disk use. The supervisor verifies the Git blobs for the affected test sources and records the native toolchain hashes and commands.

    python3 replay.py --cpython-source "$cpython" --python "$cpython/python" \
        --engine stock --layout full --work /dev/shm/environ-stock

    python3 replay.py --cpython-source "$cpython" --python "$cpython/python" \
        --engine boxed --layout retained-class --level 0 --imre "$imre" \
        --runtime "$runtime" --clang "$clang" --case test_items \
        --case test_environb --work /dev/shm/environ-first-boxed-O0

Use fresh work directories for each run. Repeat with `--level 3` and with `--engine ordinary`. Omit `--case` to run all 32. Under `--layout full` the entire original test_os.py is staged. Under `--layout retained-class` only the complete, unmodified class, its imports, and original `tearDownModule` survive the projection; original line numbers are preserved. Results from those layouts are separate.

A separate run with `--wrong` changes only the expectation in the original `test_items` assertion to the actual expected value plus `!`. The supervisor returns success for this negative control only if that single original reports FAIL, the wrong value appears in its assertion, and both environment checks succeed. In all other runs the supervisor returns success only when every selected case has a verified pass. The result file in each work directory retains raw build and case stdout/stderr, skips, failures, errors and exit statuses. A failure to import the module or complete discovery yields a diagnostic instead of case credit.
