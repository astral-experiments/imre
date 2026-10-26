# CPython time evidence

Imre exposes `time.sleep`, `time.time`, `time.gmtime`, `time.localtime`, `time.mktime` and `time.struct_time`. For example, `time.mktime(time.localtime(-1))` returns `-1.0`. Conversion uses the process's POSIX timezone, including the host's rules for missing and repeated wall times. On failure, the C library leaves a sentinel in the weekday field and Imre raises `OverflowError`.

Clock conversions yield a nine-item tuple subclass with named fields. Its `tm_zone` and `tm_gmtoff` attributes sit outside the sequence. The constructor accepts a fixed tuple or list of nine to eleven items, including caller-supplied zone and offset. Arbitrary iterables, the optional field dictionary and the keyword constructor form are outside the measured interface. The compiler diagnoses other time exports such as `ctime` and `strftime`.

## Original CPython suite

`UPSTREAM.json` pins unmodified CPython 3.14.5 source, tests and license to 5607950ef232dad16d75c0cf53101d9649d89115. The complete Linux reference runs 63 test methods with default regrtest resources. Its 60 passes and three skips form the denominator at both optimization levels.

| Outcome per level | Cases |
| --- | ---: |
| Original body, assertions and time operations in Imre with an assisted fixture | 4 |
| Original body offered to Imre and blocked at compile time | 1 |
| CPython reference only | 55 |
| Skip from the original CPython fixture | 3 |

The four passing `test.test_time.TimeTestCase` methods are `test_mktime`, `test_epoch`, `test_gmtime_without_arg` and `test_localtime_without_arg`. Imre runs the unmodified method and `setUp` bytes. CPython drives discovery and the outer lifecycle. The native adapter declares the test instance's timestamp and implements the numeric or tuple comparison through `TestCase.assertTrue`. Each original method fails when the native fixture supplies a displaced timestamp. `test_conversions` stops at the original `time.ctime` call. The reference skips come from the upstream glibc guard, a clock-setting permission check and the Darwin-only weak-linking class.

`tests/time_local.py` compares actual output with separate CPython processes under UTC and a controlled US Eastern DST rule. It covers epoch and civil round trips, DST flags and gap normalization, numeric ranges, ordered `__index__` calls and exceptions, struct_time class identity, nine sequence slots and both hidden attributes.

`werkzeug_http_date.py` uses the locked Flask wheelhouse. CPython calls the complete, unchanged Werkzeug 3.1.8 `http_date`. Imre compiles the original `isinstance` test, `mktime` assignment and final `email.utils.formatdate` call in a wrapper typed for this branch. It compares eight inputs in two timezones at O0 and O3 in ordinary and reachable-only builds. These consumer checks are separate from the upstream method counts.

## Reproduce

Run from the repository root with CPython 3.14.5 and its matching test package.

```sh
suite=benchmarks/cpython_stdlib/full/time
python3 -m test test_time
python3 benchmarks/cpython_stdlib/suite_inventory.py "$suite/UPSTREAM.json" --result /tmp/time-reference.json
cmp "$suite/reference-linux.json" /tmp/time-reference.json
python3 tests/time_local.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18
python3 "$suite/run.py" --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --result /tmp/time-native.json
cmp "$suite/evidence-linux.json" /tmp/time-native.json
python3 "$suite/werkzeug_http_date.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --wheelhouse /path/to/flask-wheelhouse
```
