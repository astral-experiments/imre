"""Run the storage contract with 'write' then 'read' against a fresh DB."""

import sys

from tracker_db import query


def main() -> None:
    mode = sys.argv[1]
    path = sys.argv[2]
    title = sys.argv[3] + " · Málaga 🙂\x00 :'); DROP TABLE item; --"
    if mode == "write":
        query(path, "CREATE TABLE item(id INTEGER PRIMARY KEY, title TEXT UNIQUE)", [])
        rows = query(path, "INSERT INTO item(title) VALUES (?) RETURNING id, title", [title])
        assert len(rows) == 1 and len(rows[0]) == 2
        assert rows[0][0] == "1" and rows[0][1] == title
        rows = query(path, "UPDATE item SET title = ? WHERE title = ? RETURNING id", ["changed", "missing"])
        assert not rows
        query(path, "CREATE TABLE refs(id INTEGER REFERENCES item(id) DEFERRABLE INITIALLY DEFERRED)", [])
        for sql in [
            "INSERT INTO refs VALUES (?) RETURNING id",
            "INSERT INTO item(title) VALUES (?)",
            "SELECT ?; DELETE FROM item",
            "INSERT INTO item(title) VALUES ('invalid') RETURNING CAST(x'ff' AS TEXT), ?",
        ]:
            try:
                query(path, sql, [title])
            except RuntimeError:
                continue
            raise AssertionError("expected rejected statement")
        for sql in ["SELECT 1\x00", "INSERT INTO item(title) VALUES ('bad row') RETURNING NULL"]:
            try:
                query(path, sql, [])
            except ValueError:
                continue
            raise AssertionError("expected rejected value")
    elif mode != "read":
        raise ValueError("expected write or read")
    rows = query(path, "SELECT id, title, '' FROM item ORDER BY id", [])
    assert len(rows) == 1 and len(rows[0]) == 3
    assert rows[0][0] == "1" and rows[0][1] == title and rows[0][2] == ""
    counts = query(path, "SELECT COUNT(*) FROM refs", [])
    assert counts[0][0] == "0"
    print(rows)
    print(counts)


main()
