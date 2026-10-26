# CPython datetime evidence

The source and license under `upstream/` are unchanged CPython v3.14.5 blobs. `UPSTREAM.json` pins the commit and records each blob ID. `Lib/test/test_datetime.py` registers the same classes twice against the pure Python and C-backed modules. Each default registration appears separately in both ledgers.

| Outcome per optimization level | Registrations |
| --- | ---: |
| Original method body, assertions and operations run natively in a CPython outer fixture | 12 |
| Original body offered to Imre, compiler diagnostic recorded | 4 |
| Original method and assertions run in CPython with native datetime replacements | 2 |
| Reference only | 1,048 |
| Skipped by original CPython fixture | 32 |
| **Default CPython total** | **1,098** |

The default Linux CPython run has 549 pure and 549 accelerated registrations, with 1,066 passes and 32 skips. Unrestricted `unittest` additionally discovers 2,508 cases from the local zone database (3,606 total). The default regrtest resource setting, `test.support.use_resources = []`, gives the 1,098 IDs used here. Counts apply to this reference environment.

`run.py` compiles the exact method bytes for eight distinct methods. The six that pass in both registrations are `TestDateOnly.test_delta_non_days_ignored`, `TestModule.test_utc_alias`, `TestTimeDelta.test_basic_attributes`, `TestTimeDelta.test_carries`, `TestTimeZone.test_class_members`, and `TestTimeZone.test_str`. For the two timezone methods, an adapter declares three fields and the unchanged native `setUp` fills them. CPython runs the outer Pure/Fast `sys.modules` swapping and unittest lifecycle. The other two methods record their real compile diagnostics in `native-linux.json`.

Both original `TestDateTime.test_replace` registrations also run under CPython. A fixture forwards all 18 replacements per registration to a compiled Imre worker, including two invalid leap-day replacements. CPython dispatches `copy.replace` and compares the fields returned by Imre. These two mixed cases are counted separately.

`focused.py` compares fixed and naive civil construction, normalization, UTC and named offset identity, integer range and rounding errors, date arithmetic, operand order, and exceptions escaping a comparison function. Its clock witness bounds each instant against real wall-clock observations in the CPython parent, including Werkzeug's certificate expressions `dt.now(timezone.utc)` and `dt.now(timezone.utc) + timedelta(days=365)`. It also checks that operations outside this slice are rejected instead of returning an incorrect value. These controls are independent of the original-case counts.

The date controls cover leap years, constructor errors, field access, identity, ISO and repr output, mixed date/datetime comparisons, and day arithmetic with negative fractional timedeltas. A `date` annotation preserves the original object when passed a datetime. `datetime.combine` accepts a date or datetime, a naive time including its fold, and an optional fixed timezone. A separate assertion control compares failure messages, custom exceptions and argument evaluation order.

`timezone_values.py` checks replacement of each datetime field, explicit removal and retention of timezone information, preservation of `fold`, and fresh result identity. For `astimezone`, the target must be an explicit fixed timezone. A naive source uses the host process's local rules, including gaps and repeated hours. `focused.py` runs the same fixture under UTC, US Eastern, Berlin, Lord Howe and Apia, including transitions of half an hour and a full day.

With the pinned Flask wheelhouse, `werkzeug_plain_date.py` calls the complete Werkzeug 3.1.8 `http_date` wrapper under CPython. Imre compiles the two original class guards, the midnight-UTC assignment and the final email formatting call reached by plain dates. It compares seven dates spanning years 1–9999 in both compilation modes at O0/O3. It also compiles the complete unchanged `werkzeug._internal._dt_as_utc` function and compares five None, naive and aware inputs, including equal-offset and identical timezone objects. The complete `http_date` wrapper also imports `time.mktime` for its struct-time input.

## Reproduce

Use CPython 3.14.5 and its original source checkout. From the repository root, after building Imre:

```sh
suite=benchmarks/cpython_stdlib/full/datetime
python3 -m test test_datetime
python3 benchmarks/cpython_stdlib/suite_inventory.py "$suite/UPSTREAM.json" --result /tmp/datetime-reference.json
cmp "$suite/reference-linux.json" /tmp/datetime-reference.json
python3 "$suite/focused.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18
python3 "$suite/run.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --cpython-source .cpython-stdlib-reference --result /tmp/datetime-native.json
cmp "$suite/native-linux.json" /tmp/datetime-native.json
python3 "$suite/werkzeug_plain_date.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --wheelhouse /path/to/flask-wheelhouse
```

`datetime.now` currently requires an explicit fixed `timezone`. A system-local target for `astimezone()` or `astimezone(None)`, custom `tzinfo` classes, date/time parsing, general timedelta arithmetic, pickling, and datetime subtraction from another datetime remain unsupported. Loading the full unchanged datetime suite under native Imre remains a separate milestone.

Existing `datetime.time` construction and comparison in timed logging handlers have a [separate native fixture](../logging/timed_lifecycle.py).
