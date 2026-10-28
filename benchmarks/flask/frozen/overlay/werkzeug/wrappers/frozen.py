"""The concrete request and response values accepted by frozen Flask views."""

from imre.json import decode
from urllib.parse import parse_qsl, unquote


class Request:
    method: str
    path: str
    query: str
    media: str
    cookie: str
    session_accessed: bool
    body: bytes

    def __init__(self, method: str, target: str, media: str, cookie: str, body: bytes) -> None:
        self.session_accessed = False
        self.method = method
        parts = target.partition("?")
        self.path = unquote(parts[0])
        self.query = parts[2]
        self.media = media
        self.cookie = cookie
        self.body = body

    def arg(self, name: str, default: str) -> str:
        for key, value in parse_qsl(self.query, keep_blank_values=True):
            if key == name:
                return value
        return default

    def integer_lists(self) -> dict[str, list[int]]:
        if self.media != "application/json":
            raise ValueError("JSON Content-Type required")
        return decode(self.body.decode("utf-8"), dict[str, list[int]])

    def cookie_value(self, name: str) -> str:
        for part in self.cookie.split(";"):
            key, _, value = part.strip().partition("=")
            if key == name:
                return value
        return ""


class Response:
    status: int
    content_type: str
    body: bytes
    allow: str
    cookie: str
    vary_cookie: bool

    def __init__(self, status: int, content_type: str, body: bytes) -> None:
        self.status = status
        self.content_type = content_type
        self.body = body
        self.allow = ""
        self.cookie = ""
        self.vary_cookie = False

    def wire(self, reason: str, head: bool) -> bytes:
        headers = ("HTTP/1.1 " + str(self.status) + " " + reason + "\r\nContent-Type: "
                   + self.content_type + "\r\nContent-Length: " + str(len(self.body))
                   + "\r\nConnection: close\r\n")
        if self.cookie:
            headers += "Set-Cookie: " + self.cookie + "\r\n"
        if self.cookie or self.vary_cookie:
            headers += "Vary: Cookie\r\n"
        if self.allow:
            headers += "Allow: " + self.allow + "\r\n"
        prefix = (headers + "\r\n").encode("ascii")
        return prefix if head else prefix + self.body
