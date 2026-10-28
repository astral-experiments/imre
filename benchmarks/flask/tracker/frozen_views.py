"""Concrete Flask view signatures for the frozen deployment."""
import json
from dataclasses import dataclass

from flask.frozen.state import AppState, json_ints
from imre.json import decode, encode
from werkzeug.wrappers.frozen import Request, Response
from domain import Issue

import domain
import tracker_table


@dataclass
class Feed:
    issues: list[Issue]


def json_text(text: str, status: int) -> Response:
    return Response(status, "application/json", text.encode("utf-8"))


def problem(status: int) -> Response:
    return json_ints({"error": status}, status)


def fields(req: Request) -> dict[str, str]:
    if req.media != "application/json":
        raise ValueError("expected JSON")
    return decode(req.body.decode("utf-8"), dict[str, str])


def login(req: Request, capture: str, state: AppState) -> Response:
    uid = domain.authenticate(fields(req))
    if not uid:
        return problem(401)
    nonce = domain.new_nonce()
    out = json_text(json.dumps({"csrf": str(nonce)}, separators=(",", ":")) + "\n", 200)
    state.save({"uid": uid, "nonce": nonce}, out)
    return out


def create(req: Request, capture: str, state: AppState) -> Response:
    status, ident = domain.create(state.session(req), fields(req))
    if not ident:
        return problem(status)
    return json_ints({"id": ident}, status)


def change(req: Request, capture: str, state: AppState) -> Response:
    status, ident = domain.change(state.session(req), capture, fields(req))
    if not ident:
        return problem(status)
    return json_ints({"id": ident}, status)


def issues(req: Request, capture: str, state: AppState) -> Response:
    uid = state.session(req).get("uid", 0)
    if not uid:
        return problem(401)
    rows = domain.list_issues(uid, req.arg("status", ""))
    return json_text(encode(Feed(rows)) + "\n", 200)


def listing(req: Request, capture: str, state: AppState) -> Response:
    uid = state.session(req).get("uid", 0)
    if not uid:
        return problem(401)
    rows = domain.list_issues(uid, req.arg("status", ""))
    html = tracker_table.render([domain.context(row) for row in rows])
    return Response(200, "text/html; charset=utf-8", html.encode("utf-8"))


def detail(req: Request, capture: str, state: AppState) -> Response:
    uid = state.session(req).get("uid", 0)
    if not uid:
        return problem(401)
    rows = domain.detail(uid, capture)
    if not rows:
        return problem(404)
    return state.render("detail", domain.context(rows[0]))
