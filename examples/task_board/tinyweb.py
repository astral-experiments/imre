"""A small typed router for the example. Handlers and transport are independent."""

import json


class Request:
    method: str
    path: str
    body: bytes
    media: str
    id: int

    def __init__(self, method: str, path: str, body: bytes, media: str) -> None:
        self.method = method
        self.path = path
        self.body = body
        self.media = media
        self.id = 0


class Response:
    status: int
    media: str
    body: bytes
    allow: str

    def __init__(self, status: int, media: str, body: bytes, allow: str = "") -> None:
        self.status = status
        self.media = media
        self.body = body
        self.allow = allow


def json_response(status: int, text: str) -> Response:
    return Response(status, "application/json", text.encode("utf-8"))


def error(status: int, message: str) -> Response:
    return json_response(status, json.dumps({"error": message}))


class View:
    def handle(self, req: Request) -> Response:
        return error(500, "no handler")


class Rule:
    method: str
    pieces: list[str]
    handler: View

    def __init__(self, method: str, pattern: str, handler: View) -> None:
        self.method = method
        self.pieces = pattern.split("/")
        self.handler = handler

    def match(self, parts: list[str]) -> int:
        if len(parts) != len(self.pieces):
            return -1
        ident = 0
        for i in range(len(parts)):
            part = parts[i]
            if self.pieces[i] == "<int:id>":
                if len(part) == 0 or len(part) > 18:
                    return -1
                for c in part:
                    if ord(c) < 48 or ord(c) > 57:
                        return -1
                ident = int(part)
            elif part != self.pieces[i]:
                return -1
        return ident


class Router:
    rules: list[Rule]

    def __init__(self) -> None:
        self.rules = []

    def get(self, pattern: str, handler: View) -> None:
        self.rules.append(Rule("GET", pattern, handler))

    def post(self, pattern: str, handler: View) -> None:
        self.rules.append(Rule("POST", pattern, handler))

    def dispatch(self, req: Request) -> Response:
        parts = req.path.split("/")
        allowed: list[str] = []
        for rule in self.rules:
            ident = rule.match(parts)
            if ident >= 0:
                if req.method == rule.method:
                    req.id = ident
                    return rule.handler.handle(req)
                allowed.append(rule.method)
                if rule.method == "GET":
                    allowed.append("HEAD")
        if len(allowed):
            return Response(
                405,
                "application/json",
                b'{"error":"method not allowed"}',
                ", ".join(allowed),
            )
        return error(404, "not found")
