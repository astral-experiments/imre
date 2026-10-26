# email.utils

`parsedate_to_datetime` returns a native `datetime` with the RFC date's fixed offset. `-0000` produces a naive datetime. `format_datetime` emits English weekday and month names, formats native fixed-offset and naive datetimes, and enforces the UTC requirement for `usegmt=True`. `formatdate` accepts a Unix timestamp or the current time and supports historical local offsets and DST through the system timezone database.

The complete, unmodified CPython 3.14.5 email test package, its data, the original date sources and the PSF license are pinned in [UPSTREAM.json](UPSTREAM.json). Linux ran 1,793 registered methods, with 1,791 passes and two original skips. The [reference ledger](reference-linux.json) records each fixture.

| Per-method execution at ordinary and reachable O0 and O3 | Result |
| --- | ---: |
| Fully native original test and fixture | 0 |
| Unchanged date-test body, native unittest assertion and date operation, CPython supplies class fixture values | 4 |
| Additional discovered original methods and assertions on CPython, native date operations | 7 |
| Additional original Minsk tests explicitly constructed through their upstream decorator, outside default Linux discovery | 2 |

The separately measured four native method bodies cover naive and fixed-offset formatting and both invalid `usegmt=True` cases. The CPython sources rely on class-level datetime constructors, which block compiling the original class fixture as a whole. The [paired occurrence ledger](evidence-linux.json) distinguishes the reference-only cases and the original skip conditions.

The [consumer results](date-linux.json) include the stock Werkzeug wheel's `parse_date` and `http_date` for six inputs each. CPython runs the two wrappers and reconstructs parsed datetime objects for their timezone operations. Imre produces the parse result and every formatted string. The [direct controls](../../../../tests/fixtures/text/email_dates.py) exercise the native calendar result, UTC validation, historical timezones, rounding, Unicode decimal digits and checked errors.

Run the following with a test-enabled CPython 3.14.5 and the seven wheels pinned in `benchmarks/flask/requirements.txt`.

```sh
python3.14 -B benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/email/UPSTREAM.json --result /tmp/email-reference.json
python3.14 -B benchmarks/cpython_stdlib/full/email/run_dates.py \
  --compiler /path/to/imre --runtime /path/to/libimre_runtime.a \
  --wheelhouse /path/to/wheels --output /tmp/email-dates.json --evidence /tmp/email-evidence.json
```
