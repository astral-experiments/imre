"""Run one parameterized SQLite statement in a short-lived transaction."""

from _tracker_sqlite import execute


def _number(packet: bytes, start: int) -> tuple[int, int]:
    value = 0
    pos = start
    while pos < len(packet) and 48 <= packet[pos] <= 57:
        value = value * 10 + packet[pos] - 48
        pos += 1
    if pos == start or pos == len(packet) or packet[pos] != 58:
        raise ValueError("invalid SQLite result frame")
    return value, pos + 1


def query(path: str, sql: str, params: list[str]) -> list[list[str]]:
    """Bind positional TEXT parameters and return INTEGER and TEXT columns as strings.

    A modifying statement may use SQLite RETURNING. A failed statement rolls back
    before the connection closes. The bytes-only extension ABI uses UTF-8 fields
    prefixed by their byte length, so empty text, NULs and separators survive.
    """
    fields: list[str] = []
    for value in params:
        fields.append(str(len(value.encode("utf-8"))) + ":" + value)
    packet = execute(path.encode("utf-8"), sql.encode("utf-8"), "".join(fields).encode("utf-8"))
    width, pos = _number(packet, 0)
    rows: list[list[str]] = []
    while pos < len(packet):
        if width == 0:
            raise ValueError("invalid SQLite result width")
        row: list[str] = []
        for _ in range(width):
            size, pos = _number(packet, pos)
            end = pos + size
            if end > len(packet):
                raise ValueError("truncated SQLite result")
            row.append(packet[pos:end].decode("utf-8"))
            pos = end
        rows.append(row)
    return rows
