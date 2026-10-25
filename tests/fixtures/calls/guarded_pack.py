class Entry:
    value: str

    def __init__(self, value: str) -> None:
        self.value = value

    @classmethod
    def from_elements(cls, path: str, hash_: str, size: str) -> "Entry":
        return cls(path + ":" + hash_ + ":" + size)

def validate(rows: list[tuple[str, str, str] | None]) -> None:
    for record_args in rows:
        if record_args is None:
            continue
        record = Entry.from_elements(*record_args)
        print(record.value)

rows: list[tuple[str, str, str] | None] = [None, ("module.py", "sha256=x", "42"), None, ("record", "", "")]
validate(rows)
