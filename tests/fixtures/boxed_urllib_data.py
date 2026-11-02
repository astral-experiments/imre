import os
import urllib.request as request


def current():
    url = "data: text/plain;charSet=UTF-8 ;base64,Y2Fmw6%0Ar%3D"
    with request.urlopen(url) as response:
        assert response.geturl() == url
        assert response.info().get_params() == [("text/plain", ""), ("charset", "UTF-8")]
        assert response.read() == b"caf\xc3\xaa"
    path, headers = request.urlretrieve("data:application/octet-stream,%00%FF")
    try:
        with open(path, "rb") as data:
            assert data.read() == b"\0\xff"
        assert headers["Content-length"] == "2"
    finally:
        request.urlcleanup()
    assert not os.path.exists(path)
    try:
        request.urlopen("data:text/plain\nX-Injected: yes,body")
    except ValueError:
        pass
    else:
        raise AssertionError("accepted a newline in a data URL media type")
