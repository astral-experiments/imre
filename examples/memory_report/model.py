from dataclasses import dataclass, field
from pathlib import Path
from typing import Self

from imre.json import decode


@dataclass(slots=True)
class MemoryItem:
    name: str
    metadata_bytes: int = 0
    fields_bytes: int = 0


@dataclass(slots=True, kw_only=True)
class MemoryReport:
    total_bytes: int
    struct_metadata_bytes: int
    struct_fields_bytes: int
    memo_metadata_bytes: int
    memo_fields_bytes: int
    structs: list[MemoryItem] = field(default_factory=list)
    queries: list[MemoryItem] = field(default_factory=list)

    @classmethod
    def from_json(cls, path: Path) -> Self:
        return decode(path.read_text(encoding="utf-8"), cls)


@dataclass(slots=True, kw_only=True)
class ProjectComparison:
    name: str
    old: MemoryReport
    new: MemoryReport

    @property
    def total_diff_bytes(self) -> int:
        return self.new.total_bytes - self.old.total_bytes


@dataclass(slots=True, frozen=True)
class ItemDifference:
    name: str
    old_bytes: int
    new_bytes: int
