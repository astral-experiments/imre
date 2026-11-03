from email.message import Message
from urllib.request import HTTPBasicAuthHandler, Request


class Opener:
    def open(self, req, timeout):
        return req.unredirected_hdrs["Authorization"], timeout


handler = HTTPBasicAuthHandler()
handler.add_parent(Opener())
handler.add_password("files", "http://example.org/private", "u", "p")
req = Request("http://example.org/private/file")
req.timeout = 2
headers = Message()
headers["WWW-Authenticate"] = 'Basic realm="files"'
assert handler.http_error_401(req, None, 401, "", headers) == ("Basic dTpw", 2)
assert handler.http_error_401(req, None, 401, "", headers) is None

headers = Message()
headers["WWW-Authenticate"] = "NTLM"
try:
    handler.http_error_401(req, None, 401, "", headers)
except ValueError as error:
    assert "NTLM" in str(error)
else:
    raise AssertionError("accepted an unsupported authentication scheme")
