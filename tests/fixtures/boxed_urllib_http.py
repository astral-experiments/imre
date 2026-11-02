def current():
    import http.client
    import socket
    from urllib import request

    assert "getproxies" in request.__all__ and "AbstractHTTPHandler" not in request.__all__
    opener = request.build_opener(request.ProxyHandler({}))
    if hasattr(http.client, "HTTPSConnection"):
        context = object()
        handler = request.HTTPSHandler(context=context)
        assert handler._context is context and "HTTPSHandler" in request.__all__
    else:
        from urllib.error import URLError
        try:
            opener.open("https://example.invalid/")
        except URLError as error:
            assert error.reason == "unknown url type: https"
        else:
            raise AssertionError("unexpected HTTPS transport")
    connection = http.client.HTTPConnection
    client, peer = socket.socketpair()

    class Connected(connection):
        def connect(self):
            self.sock = client

    try:
        http.client.HTTPConnection = Connected
        peer.sendall(b"HTTP/1.0 202 Accepted\r\nContent-Length: 3\r\n\r\nx\x00y")
        url = "http://127.0.0.1/resource?q=1#fragment"
        with opener.open(url, timeout=2) as response:
            assert response.getcode() == 202 and response.msg == "Accepted"
            assert response.geturl() == url
            assert response.read() == b"x\x00y"
        with peer.makefile("rb") as wire:
            assert wire.readline() == b"GET /resource?q=1 HTTP/1.1\r\n"
    finally:
        http.client.HTTPConnection = connection
        client.close()
        peer.close()
