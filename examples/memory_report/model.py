class MemoryItem:
    name: str
    metadata_bytes: int
    fields_bytes: int

    def __init__(self, name: str, metadata_bytes: int = 0, fields_bytes: int = 0) -> None:
        self.name = name
        self.metadata_bytes = metadata_bytes
        self.fields_bytes = fields_bytes


class MemoryReport:
    total_bytes: int
    struct_metadata_bytes: int
    struct_fields_bytes: int
    memo_metadata_bytes: int
    memo_fields_bytes: int
    structs: list[MemoryItem]
    queries: list[MemoryItem]

    def __init__(
        self,
        total_bytes: int,
        struct_metadata_bytes: int,
        struct_fields_bytes: int,
        memo_metadata_bytes: int,
        memo_fields_bytes: int,
        structs: list[MemoryItem] | None = None,
        queries: list[MemoryItem] | None = None,
    ) -> None:
        self.total_bytes = total_bytes
        self.struct_metadata_bytes = struct_metadata_bytes
        self.struct_fields_bytes = struct_fields_bytes
        self.memo_metadata_bytes = memo_metadata_bytes
        self.memo_fields_bytes = memo_fields_bytes
        self.structs = [] if structs is None else structs
        self.queries = [] if queries is None else queries


class ProjectComparison:
    name: str
    old: MemoryReport
    new: MemoryReport

    def __init__(self, name: str, old: MemoryReport, new: MemoryReport) -> None:
        self.name = name
        self.old = old
        self.new = new


class ItemDifference:
    name: str
    old_bytes: int
    new_bytes: int

    def __init__(self, name: str, old_bytes: int, new_bytes: int) -> None:
        self.name = name
        self.old_bytes = old_bytes
        self.new_bytes = new_bytes
