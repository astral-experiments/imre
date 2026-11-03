from email import message_from_string
from http.cookiejar import CookieJar
from urllib.request import HTTPCookieProcessor, Request


class Response:
    def info(self):
        return message_from_string("Set-Cookie: session=eggs; Path=/private\nSet-Cookie: secret=1; Secure; Path=/\n\n")


def current():
    jar = CookieJar()
    processor = HTTPCookieProcessor(jar)
    request = Request("http://origin.example/private/start")
    response = Response()
    assert processor.http_response(request, response) is response
    for url, expected in [
        ("http://origin.example/private/next", "session=eggs"),
        ("https://origin.example/elsewhere", "secret=1"),
        ("http://origin.example/", None),
        ("http://other.example/private/next", None),
    ]:
        follow = Request(url)
        assert processor.http_request(follow) is follow
        print(follow.get_header("Cookie"))
        assert follow.get_header("Cookie") == expected
