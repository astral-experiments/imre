from email.message import Message
from urllib.request import ProxyDigestAuthHandler, ProxyHandler, Request
class Opener:
    def open(self, request, timeout):
        return request.get_header("Proxy-authorization"), timeout
proxy = ProxyHandler({"http": "http://proxy.example:3128"})
handler = ProxyDigestAuthHandler()
handler.add_parent(Opener())
req = Request("http://origin.example/item")
req.set_proxy("proxy.example:3128", "http")
req.timeout = 7
challenge = 'Digest realm="cache", nonce="nonce"'
headers = Message()
headers["WWW-Authenticate"] = challenge
assert handler.http_error_407(req, None, 407, "", headers) is None
headers["Proxy-Authenticate"] = challenge
assert handler.http_error_407(req, None, 407, "", headers) is None
handler.add_password("cache", req.full_url, "user", "password")
authorization, timeout = handler.http_error_407(req, None, 407, "", headers)
assert authorization.startswith('Digest username="user"') and timeout == 7
assert not req.has_header("Authorization")
assert handler.retried == 0
assert proxy.proxies["http"] == "http://proxy.example:3128"
