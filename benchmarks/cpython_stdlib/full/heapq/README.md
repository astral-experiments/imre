# heapq

Runtime-loaded Python uses CPython 3.14.5's pure-Python heap implementation. The [native fixture](../../../../tests/fixtures/boxed_heapq.py) exercises min and max heaps, top-N selection from an iterator, and stable order for equal keys.

The [Linux reference run](reference-linux.json) passed all 69 occurrences in the [unmodified upstream suite](upstream/Lib/test/test_heapq.py). This includes CPython's optional C-accelerated variant and a doctest for `merge`. Imre provides the Python implementation.

The complete upstream test file and its original unittest loader both stop before discovery at O0 and O3. The first unsupported import is `_imp` in `test.support.import_helper`. In a separate fixture with supplied assertions, six of eight original method bodies pass. Two generator error-handling cases require native `try` in generators.

With a test-enabled CPython 3.14.5, run the reference with `python3.14 -IB -m unittest -q test.test_heapq`. The [manifest](UPSTREAM.json) records the Git blobs for the implementation, tests and license.
