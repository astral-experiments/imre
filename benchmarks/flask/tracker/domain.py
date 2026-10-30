"""Issue ownership, session credentials and SQL shared by both servers."""

import hashlib
import hmac
import os
import secrets
from dataclasses import dataclass

from tracker_db import query


@dataclass
class Issue:
    id: int
    title: str
    body: str
    status: str


def database() -> str:
    path = os.getenv("TRACKER_DB", "")
    if not path:
        raise ValueError("TRACKER_DB is required")
    return path


def authenticate(fields: dict[str, str]) -> int:
    username = fields.get("username", "")
    token = fields.get("token", "")
    if not username or not token:
        return 0
    rows = query(database(), "SELECT id, token_hash FROM users WHERE name = ?", [username])
    digest = hashlib.sha256(token.encode("utf-8")).hexdigest()
    if rows and hmac.compare_digest(digest, rows[0][1]):
        return int(rows[0][0])
    return 0


def new_nonce() -> int:
    nonce = 0
    for _ in range(4):
        nonce = nonce * 4294967296 + secrets.choice(range(4294967296))
    return nonce + 1


def write_error(session: dict[str, int], fields: dict[str, str]) -> int:
    if not session.get("uid", 0):
        return 401
    token = fields.get("csrf", "").encode("utf-8")
    nonce = session.get("nonce", 0)
    if not nonce or not hmac.compare_digest(token, str(nonce).encode("ascii")):
        return 403
    return 0


def create(session: dict[str, int], fields: dict[str, str]) -> tuple[int, int]:
    error = write_error(session, fields)
    if error:
        return error, 0
    title = fields.get("title", "").strip()
    body = fields.get("body", "")
    if not title or len(title) > 160 or len(body) > 8000:
        return 400, 0
    rows = query(database(), "INSERT INTO issues(owner, title, body) VALUES (?, ?, ?) RETURNING id",
                 [str(session["uid"]), title, body])
    return 201, int(rows[0][0])


def change(session: dict[str, int], ident: str, fields: dict[str, str]) -> tuple[int, int]:
    error = write_error(session, fields)
    if error:
        return error, 0
    status = fields.get("status", "")
    if status not in ("open", "closed"):
        return 400, 0
    rows = query(database(), "UPDATE issues SET status = ? WHERE id = ? AND owner = ? RETURNING id",
                 [status, ident, str(session["uid"])])
    if not rows:
        return 404, 0
    return 200, int(rows[0][0])


def list_issues(uid: int, status: str) -> list[Issue]:
    columns = "SELECT id, title, body, status FROM issues WHERE owner = ?"
    args = [str(uid)]
    if status:
        if status not in ("open", "closed"):
            raise ValueError("invalid status")
        columns += " AND status = ?"
        args.append(status)
    rows = query(database(), columns + " ORDER BY id DESC LIMIT 50", args)
    return [Issue(int(row[0]), row[1], row[2], row[3]) for row in rows]


def detail(uid: int, ident: str) -> list[Issue]:
    rows = query(database(), "SELECT id, title, body, status FROM issues WHERE owner = ? AND id = ?",
                 [str(uid), ident])
    return [Issue(int(row[0]), row[1], row[2], row[3]) for row in rows]


def context(issue: Issue) -> dict[str, str]:
    return {"id": str(issue.id), "title": issue.title, "body": issue.body, "status": issue.status}
