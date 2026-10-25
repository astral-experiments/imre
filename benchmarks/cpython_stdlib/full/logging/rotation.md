# Size-based log rotation

`RotatingFileHandler` keeps the real file stream. It checks seekability and the
current byte offset after flushing. The size estimate adds the character count
of the formatted record and its newline, as CPython does. It avoids rotating an
empty file or a special file, closes before moving backups and respects delayed
reopening. `rotation_filename` and `rotate` are overridable methods.

The pinned suite discovers ten `RotatingFileHandlerTest` occurrences on Linux.
[native_rotation.py](native_rotation.py) runs the following seven original routines
at both O0 and O3. All seven retain their original CPython hooks and cleanup.

| Original method | Body and assertions | Logging operation |
| --- | --- | --- |
| `test_file_created` | Imre | Unlink the fixture file, emit, check the new file and close |
| `test_should_not_rollover` | Imre | Empty file and zero-size limit, including an existing file |
| `test_should_rollover` | Imre | Existing append stream at its initial end position |
| `test_should_not_rollover_non_file` | Imre; CPython evaluates the original WASI guard | Open the real `/dev/null`, construct a record and check the decision |
| `test_should_not_rollover_named_pipe` | CPython | Native child opens, writes and closes the real FIFO; the original reader thread checks its bytes |
| `test_max_bytes`, `test_max_bytes_delay` | CPython | One native handler and its native records persist through all size decisions, writes and backup checks |

The last two bodies use a conditional keyword dictionary in the constructor,
`**({'delay': delay} if delay else {})` (spelled in the original as a local
`kwargs`). The [compiler work on fixed keyword arguments](https://github.com/astral-sh/imre/pull/429)
is relevant to this call shape. The other three occurrences, `test_rollover_filenames`,
`test_namer_rotator_inheritance` and `test_rotator`, still need callable handler
attributes, a locally defined handler class or compressed rotation. They remain
ordinary implementation blockers. No transport was excluded.

[rotation_values.py](rotation_values.py) checks the bytes in the active file and
its numbered backups, including Unicode messages, an explicitly repositioned
stream, and a file populated before the handler opens. This comparison adds no
test IDs. The [guarded-method runner discussion](https://github.com/astral-sh/imre/pull/508#issuecomment-6394373468)
records the exact non-file method body and host guard.
