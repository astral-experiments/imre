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

## Reading attributes and the live dictionary

The [record attribute comparison](../../../../tests/fixtures/files/logging_record_attributes.py)
checks the distinction between a missing field and a field containing `None`.
A retained field keeps its original value when a dictionary alias changes or
deletes the entry. Defaults run even when the field exists; the fixture also
copies the retained value into another record.

The production compiler and IR verifier matched CPython's 35 output lines, stderr and exit status at O0 and O3. The [comparison receipt](record-attributes-linux.json) records both runs. The [full suite](evidence-linux.json) retained all 272 occurrences per level and their original fixture and assertion roles. The direct comparison adds no upstream occurrences.

## Records delivered by loggers

The [recording-handler comparison](record_delivery.py) retains the same record
in two handlers on one logger hierarchy. Its message and mapping argument stay
available for later formatting; mutations to the mapping are visible and formatting
errors arise only when a handler asks for the message. The check also covers caller
names, paths, enums, disabled loggers, and the last-resort handler.

The production compiler and IR verifier matched CPython's 20 output lines, stderr and exit status at O0 and O3. The [comparison receipt](record-delivery-linux.json) records both runs. The [full suite](evidence-linux.json) accounts for all 272 occurrences per level. The original stream-setting test again runs its body and three identity assertions natively under CPython's setup and cleanup. The direct comparison adds no upstream occurrences.

An empty record has no `__qualname__` field. Adding that key with value `None` makes `hasattr` true; deleting it makes `hasattr` false again. The [canonical attribute correction](https://github.com/astral-sh/imre/pull/1504) and the comparison receipt cover this sequence on pinned CPython and Imre.
