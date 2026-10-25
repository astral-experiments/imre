from collections.abc import Iterable
from dataclasses import dataclass


@dataclass(frozen=True)
class Row:
    columns: Iterable[str]
    issues: Iterable[str]

    def message(self) -> str:
        return ", ".join(self.issues)


def check() -> None:
    columns = ("path", "hash", "size")
    errors = ["bad hash"]
    row = Row(columns=columns, issues=errors)
    print(row.columns is columns)
    print(row.issues is errors)
    print(isinstance(row.columns, tuple))
    print(isinstance(row.issues, list))
    print(len(row.columns))
    print(repr(row))
    print(str(row.issues))
    print(row == Row(columns, ["bad hash"]))
    print(row == Row(["path", "hash", "size"], ["bad hash"]))
    for error in row.issues:
        if error == "bad hash":
            errors.append("late")
        print(error)
    print(row.message())
    for name in row.columns:
        print(name)
    tuple_row = Row(columns=(), issues=("bad hash", "late"))
    print(bool(tuple_row.columns))
    print(tuple_row.message())
    print(Row([], ()))
    print(len(Row([], ()).issues))


check()
