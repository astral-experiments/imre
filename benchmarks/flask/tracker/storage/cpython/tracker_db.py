"""Reference adapter using CPython's stdlib sqlite3."""

import sqlite3
from contextlib import closing


def query(path: str, sql: str, params: list[str]) -> list[list[str]]:
    if not path or "\0" in path or "\0" in sql:
        raise ValueError("invalid database path or SQL")
    try:
        with closing(sqlite3.connect(path, isolation_level=None)) as db:
            db.execute("PRAGMA foreign_keys=ON")
            db.execute("BEGIN")
            rows = []
            for row in db.execute(sql, params):
                if any(type(field) not in (int, str) for field in row):
                    raise ValueError("SQLite results require INTEGER or TEXT columns")
                rows.append([str(field) for field in row])
            db.execute("COMMIT")
            return rows
    except sqlite3.Error as exc:
        raise RuntimeError(str(exc)) from exc
