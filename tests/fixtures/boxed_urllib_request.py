import io
from urllib.error import HTTPError
from urllib.request import Request, pathname2url, proxy_bypass_environment, url2pathname
from urllib.response import addclosehook, addinfourl


def current():
    request = Request("http://Example.Com:80/p?q#f", b"x", {"X-Test": "yes"})
    print(request.get_method(), request.origin_req_host, request.get_full_url(), request.header_items(), request.selector)
    request.add_header("Content-Length", "1")
    request.data = None
    print(request.get_method(), request.has_header("Content-length"))
    print(pathname2url("/a b", add_scheme=True), url2pathname("file:///a%20b", require_scheme=True))
    print(url2pathname("//127.0.0.1/local", resolve_host=True))
    print(proxy_bypass_environment("v.example.com:80", {"no": "example.com"}))
    data = io.BytesIO(b"abc")
    headers = {"Q": "V"}
    with addinfourl(data, headers, "file:/f", 200) as response:
        print(response.read(2), response.geturl(), response.info(), response.getcode(), response.status)
    print(data.closed)
    calls = []
    data = io.BytesIO(b"x")
    response = addclosehook(data, calls.append, "closed")
    response.close()
    response.close()
    print(data.closed, calls)
    with HTTPError("http://x", 500, "Oops", headers, io.BytesIO(b"bad")) as error:
        print(str(error), error.geturl(), error.headers, error.reason, error.read())
