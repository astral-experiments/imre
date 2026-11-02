import ftplib
import io
import socket
from urllib import request
from urllib.error import URLError


def current():
    # Decode a live FTP control response through the socket's owned text reader.
    left, right = socket.socketpair()
    with right:
        right.sendall(b"220-first line\r\n220 ready\r\n550 unavailable\r\n")
    ftp = ftplib.FTP()
    ftp.sock = left
    ftp.file = left.makefile("r", encoding=ftp.encoding)
    try:
        assert ftp.getresp() == "220-first line\n220 ready"
        try:
            ftp.getresp()
        except ftplib.error_perm as error:
            assert str(error) == "550 unavailable"
        else:
            raise AssertionError("unexpected FTP success")
    finally:
        ftp.close()
    assert left.fileno() == -1

    class Handler(request.FTPHandler):
        def connect_ftp(self, user, passwd, host, port, dirs, timeout):
            assert (user, passwd, host, port, dirs, timeout) == (
                "user", "pass", "127.0.0.1", 2121, ["some dir"], 3)
            return self

        def retrfile(self, name, mode):
            assert (name, mode) == ("file.txt", "A")
            return io.BytesIO(b"abc"), 3

    opener = request.build_opener(request.ProxyHandler({}), Handler)
    url = "ftp://user:pass@127.0.0.1:2121/some%20dir/file.txt;type=a"
    with opener.open(url, timeout=3) as result:
        assert result.geturl() == url
        assert result.headers["Content-length"] == "3"
        assert result.read() == b"abc"
    assert result.closed

    try:
        opener.open("ftp:///file.txt")
    except URLError as error:
        assert error.reason == "ftp error: no host given" and error.filename is None
    else:
        raise AssertionError("accepted missing FTP host")
