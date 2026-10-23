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


rows = [Row(Path("a/file"), ""), Row(Path("b/file"), "second"), Row(Path("/empty"), None)]
print(tags(rows)[("file", 3)])
print(total(rows))
print(first(rows))
for row in rows:
    print(f"<{row.tag!s}>")
