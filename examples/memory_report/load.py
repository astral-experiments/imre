from pathlib import Path

from .model import MemoryReport


def load_report(path: Path) -> MemoryReport:
    return MemoryReport.from_json(path)


def load_directory(directory: Path) -> dict[str, MemoryReport]:
    reports: dict[str, MemoryReport] = {}
    for path in directory.glob("*.json"):
        reports[path.stem] = load_report(path)
    return reports
