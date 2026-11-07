from contextlib import suppress


class Batch(ExceptionGroup):
    def derive(self, exceptions):
        return Batch(self.message, exceptions)


class InvalidNotes:
    def __getitem__(self, index):
        return ("import failed",)[index]

    def __len__(self):
        raise invalid_length


def current():
    from types import GenericAlias
    from typing import TypeVar

    item = TypeVar("item")
    for origin in (BaseExceptionGroup, ExceptionGroup, Batch):
        alias = origin[item]
        assert type(alias) is GenericAlias and alias.__origin__ is origin
        assert alias.__args__ == (item,) and alias[ValueError] == origin[ValueError]

    class ReportedException(BaseException):
        @property
        def __class__(self):
            if self.args:
                raise self.args[0]
            return Exception

    # Match leaves by actual type, but choose the derived group type using isinstance.
    leaf = ReportedException()
    group = BaseExceptionGroup("mixed", [leaf, KeyboardInterrupt()])
    part = group.subgroup(ReportedException)
    assert type(part) is ExceptionGroup and part.exceptions[0] is leaf
    assert group.subgroup(Exception) is None

    invalid_class = LookupError("invalid reported class")
    try:
        BaseExceptionGroup("mixed", [KeyboardInterrupt(), ReportedException(invalid_class)])
    except LookupError as exc:
        assert exc is invalid_class
    else:
        raise AssertionError("discarded the exception from __class__")


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

if __name__ == "__main__":
    current()
