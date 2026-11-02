import os
import tempfile
from email.message import Message
from urllib import request
from urllib.error import ContentTooShortError
from urllib.response import addinfourl


def current():
    fd, name = tempfile.mkstemp()
    os.close(fd)
    with open(name, "wb") as src:
        src.write(b"x" * 8193)

    class Opener:
        def open(self, url, data, timeout):
            headers = Message()
            headers["Content-Length"] = str(8194 if url.endswith("short") else 8193)
            self.last = addinfourl(open(name, "rb"), headers, url)
            return self.last

    opener = Opener()
    previous = request._opener
    request.install_opener(opener)
    target = name + ".copy"
    try:
        calls = []
        def report(block, size, total):
            calls.append((block, size, total))
        filename, _ = request.urlretrieve("test:full", target, report)
        assert filename == target and opener.last.closed
        assert calls == [(0, 8192, 8193), (1, 8192, 8193), (2, 8192, 8193)]
        try:
            request.urlretrieve("test:short")
        except ContentTooShortError as exc:
            partial, headers = exc.content
            assert headers["Content-Length"] == "8194" and opener.last.closed
        else:
            raise AssertionError("accepted truncated input")
        with open(partial, "rb") as src:
            assert src.read() == b"x" * 8193
        request.urlcleanup()
        assert not os.path.exists(partial)
        with open(target, "rb") as src:
            assert src.read() == b"x" * 8193
    finally:
        request.urlcleanup()
        request.install_opener(previous)
        os.unlink(name)
        if os.path.exists(target):
            os.unlink(target)
