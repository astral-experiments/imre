"""State shared by all views of one frozen application."""

import json
import time
from imre.json import decode
from itsdangerous.frozen import FrozenSigner
from jinja2.textprogram import TextProgram
from werkzeug.wrappers.frozen import Request, Response


class AppState:
    templates: dict[str, TextProgram]
    signer: FrozenSigner
    cookie_name: str

    def __init__(self, secret: bytes, salt: bytes, max_age: int, cookie_name: str) -> None:
        self.templates = {}
        self.signer = FrozenSigner(secret, salt, max_age)
        self.cookie_name = cookie_name

    def render(self, name: str, context: dict[str, str]) -> Response:
        text = self.templates[name].render(context)
        return Response(200, "text/html; charset=utf-8", text.encode("utf-8"))

    def session(self, req: Request) -> dict[str, int]:
        token = req.cookie_value(self.cookie_name)
        if token:
            try:
                payload = self.signer.open(token, int(time.time()))
                return decode(payload.decode("utf-8"), dict[str, int])
            except ValueError:
                pass
        return {}

    def save(self, data: dict[str, int], response: Response) -> None:
        token = self.signer.sign(json.dumps(data, separators=(",", ":")).encode("utf-8"), int(time.time()))
        response.cookie = self.cookie_name + "=" + token + "; HttpOnly; Path=/"


def json_ints(data: dict[str, int], status: int) -> Response:
    return Response(status, "application/json", (json.dumps(data, separators=(",", ":")) + "\n").encode("utf-8"))


def json_bools(data: dict[str, bool], status: int) -> Response:
    return Response(status, "application/json", (json.dumps(data, separators=(",", ":")) + "\n").encode("utf-8"))
