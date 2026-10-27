# difflib

With `--runtime-imports`, Imre loads CPython 3.14.5's original `difflib.py`. The [native fixture](../../../../tests/fixtures/boxed_difflib.py) compares matching blocks, opcodes, ratios and suggestions. The default checked compiler also supports [`ndiff` on sequences of lines](../../../../tests/fixtures/iterables/ndiff.py).

The [Linux reference run](reference-linux.json) passed all 58 occurrences in the pinned upstream suite. These comprise 38 unit tests and 20 doctest groups. The [manifest](UPSTREAM.json) identifies the original module, test, expected HTML and license.

The complete upstream file stops during import of `test.support` before native test discovery. Separately, 19 of 38 original unit-test bodies passed at O0 and O3 with supplied assertions. An example runner compared the 75 original interactive inputs with CPython's expected output. At both optimization levels, 54 inputs matched and 11 entire groups passed, including all five `get_close_matches` examples. These component runs use separate fixtures.

Run the original suite with a test-enabled CPython 3.14.5 and `python3.14 -IB -m unittest -q test.test_difflib`.
