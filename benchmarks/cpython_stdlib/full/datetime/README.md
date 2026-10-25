# CPython datetime evidence

The source and license under `upstream/` are unchanged CPython v3.14.5 blobs. `UPSTREAM.json` pins the commit and records each blob ID. `Lib/test/test_datetime.py` registers the same classes twice against the pure Python and C-backed modules. Each default registration appears separately in both ledgers.

| Outcome per optimization level | Registrations |
| --- | ---: |
| Original method body, assertions and operations run natively; CPython provides the outer fixture | 4 |
| Original body offered to Imre, compiler diagnostic recorded | 10 |
| Reference only | 1,052 |
| Skipped by original CPython fixture | 32 |
| **Default CPython total** | **1,098** |

The default Linux CPython run has 549 pure and 549 accelerated registrations, with 1,066 passes and 32 skips. Unrestricted `unittest` additionally discovers 2,508 cases from the local zone database (3,606 total). The default regrtest resource setting, `test.support.use_resources = []`, gives the 1,098 IDs used here. Counts apply to this reference environment.

`run.py` compiles the exact method bytes for seven distinct methods. The four native successes are `TestTimeDelta.test_basic_attributes` and `TestTimeZone.test_str`, each registered twice. The latter exercises five timezone string/name results and runs the exact original `setUp`: its adapter first declares the three fields needed for static layout, which the original fixture then fills. CPython still runs the outer Pure/Fast `sys.modules` swapping and unittest lifecycle for both methods. Each other selected method records its real compile diagnostic; support for basic UTC identity in a separate control does not credit upstream `test_utc_alias` while native `assertIs` blocks it. See `native-linux.json` for the original IDs and evidence roles.

`focused.py` compares fixed and naive civil construction, normalization, UTC and named offset identity, integer range and rounding errors, date arithmetic, operand order, and exceptions escaping a comparison function. Its clock witness bounds each instant against real wall-clock observations in the CPython parent, including Werkzeug's certificate expressions `dt.now(timezone.utc)` and `dt.now(timezone.utc) + timedelta(days=365)`. It also checks that operations outside this slice are rejected instead of returning an incorrect value. These controls are independent of the original-case counts.

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
```

`datetime.now` currently requires an explicit fixed `timezone`. Local/DST rules, custom `tzinfo` classes, date/time parsing, general timedelta arithmetic, pickling, and datetime subtraction from another datetime are outside this slice. Loading the full unchanged datetime suite under native Imre remains a separate milestone.

Existing `datetime.time` construction and comparison in timed logging handlers have a [separate native fixture](../logging/timed_lifecycle.py).
