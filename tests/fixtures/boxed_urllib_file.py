import email.utils
import os
import tempfile
from urllib import request
from urllib.error import URLError


def current():
    fd, path = tempfile.mkstemp(suffix=" café #%.txt")
    os.close(fd)
    try:
        body = b"\x00\xffread\r\n"
        with open(path, "wb") as stream:
            stream.write(body)
        url = request.pathname2url(path, add_scheme=True)
        assert "%23%25" in url
        local = url.replace("file:///", "file://localhost/", 1) + "?discard#fragment"
        handler = request.FileHandler()
        with handler.file_open(request.Request(local)) as response:
            assert response.geturl() == url
            assert response.headers is response.info()
            assert response.headers["Content-type"] == "text/plain"
            assert response.headers["Content-length"] == str(len(body))
            assert response.headers["Last-modified"] == email.utils.formatdate(
                os.stat(path).st_mtime, usegmt=True)
            assert response.read() == body
        assert response.closed
        with request.urlopen(url) as response:
            assert response.read() == body
        os.unlink(path)
        try:
            request.urlopen(url)
        except URLError as error:
            assert isinstance(error.reason, FileNotFoundError) and error.filename == path
        else:
            raise AssertionError("opened a missing file")
        if os.name != "nt":
            try:
                request.urlopen("file://192.0.2.1/remote")
            except URLError as error:
                assert error.reason == "file:// scheme is supported only on localhost"
            else:
                raise AssertionError("opened a remote POSIX file URL")
    finally:
        if os.path.exists(path):
            os.unlink(path)

    class Override(request.FileHandler):
        def file_open(self, req):
            return req.timeout, req.full_url

    opener = request.build_opener(Override)
    assert opener.open("file:custom", timeout=3) == (3, "file:custom")

    class Recorder:
        def open(self, url, data, timeout):
            return url, data, timeout

    original = request._opener
    try:
        request.install_opener(Recorder())
        req = request.Request("http://example.invalid/test")
        assert request.urlopen(req, b"data", 7) == (req, b"data", 7)
    finally:
        request.install_opener(original)
