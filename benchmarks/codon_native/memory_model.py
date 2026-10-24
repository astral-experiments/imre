"""Codon's native records and JSON reader for the shared memory report renderer."""

import json
from pathlib import Path


@dataclass
class MemoryItem:
    name: str
    metadata_bytes: int
    fields_bytes: int


@dataclass
class MemoryReport:
    total_bytes: int
    struct_metadata_bytes: int
    struct_fields_bytes: int
    memo_metadata_bytes: int
    memo_fields_bytes: int
    structs: list[MemoryItem]
    queries: list[MemoryItem]


@dataclass
class ProjectComparison:
    name: str
    old: MemoryReport
    new: MemoryReport

    @property
    def total_diff_bytes(self) -> int:
        return self.new.total_bytes - self.old.total_bytes


@dataclass
class ItemDifference:
    name: str
    old_bytes: int
    new_bytes: int


def items(data: json.JSON, key: str) -> list[MemoryItem]:
    result: list[MemoryItem] = []
    if key in data:
        for row in data[key].values():
            result.append(
                MemoryItem(
                    row["name"].as_str(),
                    row.get("metadata_bytes", 0).as_int(),
                    row.get("fields_bytes", 0).as_int(),
                )
            )
    return result


def load_report(path: Path) -> MemoryReport:
    with path.open() as stream:
        data = json.load(stream)
    return MemoryReport(
        data["total_bytes"].as_int(),
        data["struct_metadata_bytes"].as_int(),
        data["struct_fields_bytes"].as_int(),
        data["memo_metadata_bytes"].as_int(),
        data["memo_fields_bytes"].as_int(),
        items(data, "structs"),
        items(data, "queries"),
    )
