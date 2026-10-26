# base64

The native `b64encode` function encodes immutable bytes. The native `b64decode` function decodes immutable bytes or ASCII text and supports strict validation. Both delegate to the same native data conversion used by `binascii`. The wrappers preserve the order in which CPython validates the input, checks the two-byte alternate alphabet and evaluates the truth of `validate`. The decoder also accepts an ASCII string as the alternate alphabet. The [public fixture](../../../../tests/fixtures/text/base64_basic.py) checks explicit `None`, keyword ordering, repeated alphabet bytes, invalid alternates and exception classes.

## Original CPython tests

The [pinned upstream files](UPSTREAM.json) and [Linux reference](reference-linux.json) cover every one of the 41 original test methods. CPython passes all 41 with its default resource settings.

The complete original `BaseXYTestCase.test_b64decode_padding_error` runs under native `unittest.main()` at O0 and O3 with its original class header, default fixture and real `assertRaises`. It verifies that invalid bytes and invalid ASCII text both raise `binascii.Error`. Two independent input changes prove the assertion is effective. A valid input causes an assertion failure. Non-ASCII input produces an unexpected plain `ValueError` and an error result.

CPython runs the original `test_b64decode_altchars` body with all 28 product calls on Imre. It also runs three original methods whose operations are split between Imre and CPython. The runner counts calls to other aliases and mutable or view-backed buffers as host operations. The remaining 36 cases have reference results only. [Per-occurrence evidence](evidence-linux.json).

## Werkzeug

The optional wheel test loads the seven pinned Flask milestone wheels and verifies the source digest of Werkzeug's original `Authorization` module. It runs `Authorization.from_header` and `to_header` for the [16 recorded inputs](werkzeug-basic.json). The original methods and UTF-8 conversion run on CPython, while the 20 calls to `base64.b64decode` and `base64.b64encode` run in the native executable. Malformed padding yields `None`, and `Basic !!!!` renders as `Basic Og==`. `Basic /w==` yields `None` after failing UTF-8 conversion. `Basic é` propagates plain `ValueError`.

The [runner](run.py) also compares 456 payload, alternate alphabet and strictness combinations against CPython at each optimization level. It checks results or the exception class, message and arguments. Implicit exception chaining from CPython's private buffer converter remains outside this measurement. Mutable buffers, user-defined buffer providers and general alternate alphabets supplied as arbitrary Python objects are outside the native input contract.

Run with CPython 3.14.5 and its test package. Supply the offline wheelhouse to include the actual Werkzeug calls, or omit it for the CPython suite and codec checks. Add `--reachable-only` for that compiler mode.

```sh
python3.14 benchmarks/cpython_stdlib/full/base64/run.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --wheelhouse /path/to/wheelhouse --result /tmp/base64-evidence.json
```
