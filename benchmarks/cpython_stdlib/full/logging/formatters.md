# Formatter execution

Eight of the eleven `test.test_logging.FormatterTest` methods run in this gate.
The shared fixture runner preserves the CPython module/class/instance hooks.
At O0 and O3 the host-driven route makes 5,165 native calls per level. The
native implementation creates records and formatters, validates and applies
three styles, captures message/asctime fields, reads live defaults and selects
the clock converter. The original host assertions check the returned values
and exceptions.

| Methods | Body | Assertions | Logging operations |
| --- | --- | --- | --- |
| `test_percent`, `test_braces`, `test_dollars`, `test_defaults_parameter`, `test_format_validate`, `test_invalid_style`, `test_time` | CPython | CPython | Imre |
| `test_issue_89047` | Imre | Imre | Imre: 2,500 created and formatted records |

The last method also passes via the host-driven route. Count the ID once.
The shared runner hashes its exact definition; the compiled child sleeps and
performs its string interpolation and membership assertions natively.
`formatter_values.py` is a separate CPython differential for aliases,
missing-field errors, mutated defaults and historical UTC formatting. It adds
no upstream ID.

The [shared string.Template implementation](https://github.com/astral-sh/imre/pull/726)
also compiles CPython's unchanged `logging.StringTemplateStyle.validate`.
Its nine format comparisons at O0 and O3 exercise `Template.pattern` in that
method. They are method evidence, separate from the 272 logging test occurrences.

Both `BufferingFormatterTest` methods also run their unchanged bodies and
assertions natively at O0 and O3. The child constructs the original two records
using the upstream `setUp` definition. The custom case compiles the upstream
header and footer overrides and uses a second line formatter. The CPython
module, class and instance hooks still run outside the child.

```sh
python -B benchmarks/cpython_stdlib/full/logging/native_formatters.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /usr/bin/clang-18 --opt-level 0 --result /tmp/formatters-O0.json
```

Repeat with `--opt-level 3`. The remaining formatter tests include subclass
overrides, a mocked nanosecond clock, and import-time timing in a subprocess.
Rendering uncached exception tracebacks also needs runtime traceback capture.
Whole-module import is a separate gate: the pinned suite cannot yet start in
Imre, and these selected methods do not count as a native suite run.
