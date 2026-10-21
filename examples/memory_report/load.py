from .model import MemoryReport


def load_report(path: str) -> MemoryReport:
    return MemoryReport.from_json(path)
