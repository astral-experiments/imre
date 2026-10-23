from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Row:
    path: Path
    tag: str | None


def tags(rows: Sequence[Row]) -> dict[tuple[str, int], str]:
    return {
        (row.path.name, len(rows)): row.tag
        for row in rows
        if row.tag is not None
    }


def total(rows: Sequence[Row]) -> int:
    size = len(rows)
    for row in rows:
        size += len(row.path.as_posix())
    return size


def first(rows: Sequence[Row]) -> str:
    return rows[0].path.as_posix()


def keyed_tags(rows: Sequence[Row]) -> dict[str, str]:
    return {row.tag: row.tag for row in rows if row.tag is not None}


@dataclass
class Score:
    value: int | None


def score_total(scores: Sequence[Score]) -> int:
    return sum([score.value for score in scores if score.value is not None])


def scopes(values: Sequence[int]) -> int:
    def append_to_copy(values: list[int]) -> int:
        values.append(8)
        return len(values)

    def outer_size() -> int:
        return len(values)

    copied = [values + 1 for values in values]
    print(all(values > 0 for values in values))
    return append_to_copy(copied) + outer_size()


rows = [Row(Path("a/file"), ""), Row(Path("b/file"), "second"), Row(Path("/empty"), None)]
print(tags(rows)[("file", 3)])
print(total(rows))
print(first(rows))
print(keyed_tags(rows)["second"])
print(score_total([Score(None), Score(-3), Score(1 << 100)]))
print(scopes([2, 3]))
for row in rows:
    print(f"<{row.tag!s}>")
