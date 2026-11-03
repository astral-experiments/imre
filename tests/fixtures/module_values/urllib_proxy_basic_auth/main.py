from email.message import Message
from urllib.request import ProxyBasicAuthHandler, Request


class Opener:
    def open(self, request, timeout):
        return request.get_header("Proxy-authorization"), timeout


handler = ProxyBasicAuthHandler()
handler.add_password("cache", "proxy.example:3128", "u", "p")
handler.add_parent(Opener())
request = Request("http://origin.example/private")
request.set_proxy("proxy.example:3128", "http")
request.timeout = 3

headers = Message()
headers["WWW-Authenticate"] = 'Basic realm="cache"'
assert handler.http_error_407(request, None, 407, "", headers) is None
headers["Proxy-Authenticate"] = 'Basic realm="cache"'
assert handler.http_error_407(request, None, 407, "", headers) == ("Basic dTpw", 3)
assert handler.http_error_407(request, None, 407, "", headers) is None
assert not request.has_header("Authorization")
