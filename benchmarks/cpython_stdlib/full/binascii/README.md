# binascii

The native module encodes immutable bytes with `b2a_base64(data, /, *, newline=True)`. Its decoder accepts immutable bytes and ASCII strings, including strict validation with `a2b_base64(data, /, *, strict_mode=False)`. It preserves CPython 3.14.5's padding rules and the class and message of failures. For example, the default decoder accepts `b"ab==c="` and returns `b"i\xb7"`. Strict mode rejects those same bytes with `binascii.Error("Excess data after padding")`. Non-ASCII text raises plain `ValueError`.

`binascii.Error` derives from `ValueError`. The module's other exception, `binascii.Incomplete`, derives directly from `Exception`. The runtime uses the same hierarchy for catching and subclass checks. Mutable buffers, memoryviews, checksums, and the module's other encodings remain outside this slice.

## CPython evidence

The [pinned source manifest](UPSTREAM.json) verifies the complete unchanged `test_binascii.py`. At CPython's default resource settings, with Hypothesis absent, the [Linux reference](reference-linux.json) records 93 cases, including 17 upstream skips.

At O0 and O3, the complete original `BinASCIITest.test_base64valid` and `test_b2a_base64_newline` bodies call the public Imre functions and compare their byte results natively. The generated fixture supplies the original `rawdata`, an identity-preserving bytes constructor, and a byte equality assertion. A negative control checks the assertion's failure. Four additional original methods retain their CPython fixtures and assertions and run every codec call on the native worker. They cover noise, strict mode, excess data and incorrect padding. The three inherited mutable-buffer variants are accounted for separately in the [paired occurrence ledger](evidence-linux.json).

The same runner compares 8,856 additional public codec calls against CPython per compiler configuration. These cover every combination of up to five characters from an alphabet that includes padding and noise, all 256 byte values in a surrounding quartet, non-ASCII strings, and encoding lengths from zero to 256. These calls are outside the 93-case test count.

Run from the repository root with the pinned CPython 3.14.5 interpreter and its unchanged test package:

```sh
python3.14 benchmarks/cpython_stdlib/full/binascii/run.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /path/to/clang --result /tmp/binascii-evidence.json
```

Add `--reachable-only` to compare that compiler mode. The [standalone codec fixture](../../../../tests/fixtures/text/binascii_base64.py) also checks exact exception types, constructor representation, exception hierarchy, and the order of input conversion and a user-defined keyword flag's `__bool__`.
