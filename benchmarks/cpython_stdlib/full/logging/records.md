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

## Logger.makeRecord and extra

The [focused replay](make-record-extra-linux.json) runs both original
`LoggerTest` methods under CPython’s module, class and instance fixtures at
O0 and O3. Record creation does not dispatch an event to any handler.

| Original method | Body and assertions | Native logging operations |
| --- | --- | --- |
| `test_make_record_with_extra_no_overwrite` | Imre runs the complete unchanged body and checks dictionary membership on its original operands. | Construct the record and expose its live dictionary. |
| `test_make_record_with_extra_overwrite` | CPython runs the unchanged loop and `assertRaises`; the native compiler stops at `tuple(rv.__dict__.keys())`. | One native child creates the default record, enumerates its 21 fields, and rejects attempts to overwrite those fields, `message`, and `asctime`. |

The [dictionary control](make_record_values.py) checks alias mutation, copied
records, protected names, argument evaluation order and catchable errors. The
[operand control](make_record_operands.py) checks paths, Enum and StrEnum values,
and tagged integers, including zero. Both compare the complete process output
against the pinned interpreter and add no test IDs. These checks exercise the
default record factory.
