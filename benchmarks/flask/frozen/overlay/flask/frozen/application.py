"""Dispatch a frozen Werkzeug map to compiled Flask views."""

import sys
from flask.frozen.state import AppState
from werkzeug.routing.frozen import FrozenMap
from werkzeug.wrappers.frozen import Request, Response


class Application:
    state: AppState
    rules: FrozenMap
    errors: dict[int, bytes]
    reasons: dict[int, str]
    methods: list[str]
    automatic_options: list[bool]

    def __init__(self, state: AppState) -> None:
        self.state = state
        self.rules = FrozenMap()
        self.errors = {}
        self.reasons = {}
        self.methods = []
        self.automatic_options = []

    def error(self, status: int) -> Response:
        return Response(status, "text/html; charset=utf-8", self.errors[status])

    def view(self, index: int, req: Request, capture: str) -> Response:
        raise ValueError("unregistered view")

    def dispatch(self, req: Request) -> Response:
        index, capture = self.rules.match(req.method, req.path)
        if index >= 0 and not (req.method == "OPTIONS" and self.automatic_options[index]):
            try:
                out = self.view(index, req, capture)
            except KeyError as exc:
                print("KeyError: " + str(exc), file=sys.stderr)
                out = self.error(500)
            except ValueError:
                out = self.error(400)
            out.vary_cookie = req.session_accessed
            return out
        allowed: list[str] = []
        for method in self.methods:
            other, _ = self.rules.match(method, req.path)
            if other >= 0:
                allowed.append(method)
        if index >= 0:
            out = Response(200, "text/html; charset=utf-8", b"")
        elif allowed:
            out = self.error(405)
        else:
            return self.error(404)
        out.allow = ", ".join(allowed)
        return out
