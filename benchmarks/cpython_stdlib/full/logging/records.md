# Log records

Both executions below keep the original CPython module, class and instance
fixture hooks. Use the test-enabled 3.14.5 interpreter and the paths documented
in the shared suite runner:

```sh
python native_records.py --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /usr/bin/clang-18 --opt-level 0 --result /tmp/logging-records-O0.json
```

Repeat at level 3. The runner verifies the logging manifest and reconciles
discovery with the 272-ID reference inventory before selecting these methods.

| Original method in `test.test_logging.LogRecordTest` | Body | Tested operation | Assertions |
| --- | --- | --- | --- |
| `test_str_rep` | Imre, byte-for-byte definition | Native record creation and representation | Native string prefix and suffix checks |
| `test_optional` | CPython | Native record creation, metadata reads and four flags in one child process | Original CPython assertions |

The driver does not create Python logging records on the host. The separate
`record_values.py` differential covers mutation, repeated access to the same
record, copied extra fields, retained arguments, and formatting errors. It contributes
no additional upstream ID. The asyncio task-name and multiprocessing child-process
tests still require their respective runtime contexts; neither is excluded.

## Records delivered by loggers

The [recording handler comparison](record_delivery.py)
retains a record in two handlers on the same hierarchy and compares both streams with
pinned CPython at O0 and O3. It checks that the original format and mutable argument
mapping survive delivery, that formatting errors wait for `getMessage()`, and that the
caller, paths and both enum display styles use their original Python values. A disabled
logger still evaluates its arguments; the last-resort handler only renders eligible events.
These direct comparisons are outside the 272 original occurrences.
