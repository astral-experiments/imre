def current():
    import socket
    from http.client import HTTPConnection

    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        host, port = listener.getsockname()
        conn = HTTPConnection(host, port)
        try:
            conn.request("GET", "/resource?x=1")
            peer, _ = listener.accept()
            with peer:
                with peer.makefile("rb") as wire:
                    assert wire.readline() == b"GET /resource?x=1 HTTP/1.1\r\n"
                    peer.sendall(
                        b"HTTP/1.1 201 Created\r\nFrom relay\r\nX-Note: first\r\n kept\r\n"
                        b"From another relay\r\n ignored\r\nContent-Length: 3\r\n"
                        b"malformed line\r\nIgnored: yes\r\n\r\na\x00b"
                    )
                    with conn.getresponse() as response:
                        assert (response.status, response.reason) == (201, "Created")
                        assert response.headers.items() == [
                            ("X-Note", "first\r\n kept"), ("Content-Length", "3"),
                        ]
                        assert response.read(1) == b"a"
                        assert response.read() == b"\x00b"
        finally:
            conn.close()

    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        address = reserved.getsockname()
    try:
        socket.create_connection(address)
    except ConnectionRefusedError:
        pass
    else:
        raise AssertionError("connected to a closed port")
