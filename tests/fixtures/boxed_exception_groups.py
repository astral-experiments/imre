from contextlib import suppress


class Batch(ExceptionGroup):
    def derive(self, exceptions):
        return Batch(self.message, exceptions)


class InvalidNotes:
    def __getitem__(self, index):
        return ("import failed",)[index]

    def __len__(self):
        raise invalid_length


first = ValueError("bad header")
second = ValueError("bad record")
missing = KeyError("missing file")
nested = Batch("worker", [second, missing])
errors = [first, nested]
batch = Batch("batch", errors)
batch.add_note("import failed")

matched, rest = batch.split(ValueError)
assert type(matched) is type(rest) is Batch
assert matched.exceptions[0] is first
assert type(matched.exceptions[1]) is Batch
assert matched.exceptions[1].exceptions == (second,)
assert rest.exceptions[0].exceptions == (missing,)
assert rest.__notes__ == batch.__notes__ and rest.__notes__ is not batch.__notes__
assert batch.subgroup(lambda exc: exc is nested).exceptions == (nested,)
assert batch.subgroup(TypeError) is None
assert batch.split(Batch) == (batch, None)

# Group members are independent of the original mutable sequence and of args.
before = repr(batch)
errors.clear()
assert repr(batch) == before
batch.args = ()
assert batch.exceptions == (first, nested)
assert str(batch) == "batch (2 sub-exceptions)"
assert type(BaseExceptionGroup("cancel", [KeyboardInterrupt()])) is BaseExceptionGroup
assert type(BaseExceptionGroup("retry", [missing])) is ExceptionGroup

try:
    with suppress(ValueError):
        raise batch
except Batch as remainder:
    assert remainder.exceptions[0].exceptions == (missing,)
else:
    raise AssertionError("suppress discarded the unmatched error")

invalid_length = ValueError("invalid notes length")
batch.__notes__ = InvalidNotes()
for partition in (batch.subgroup, batch.split):
    try:
        partition(ValueError)
    except ValueError as exc:
        assert exc is invalid_length
    else:
        raise AssertionError("discarded the exception from __len__")
