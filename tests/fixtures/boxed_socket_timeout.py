def current():
    import _socket
    import socket

    assert socket.timeout is _socket.timeout is TimeoutError

    reader, writer = socket.socketpair()
    with reader, writer, reader.makefile('rb') as stream:
        assert reader.gettimeout() is None and reader.getblocking()
        reader.settimeout(0)
        assert reader.gettimeout() == 0.0 and not reader.getblocking()
        reader.setblocking(True)
        assert reader.gettimeout() is None

        reader.settimeout(0.025)
        assert reader.gettimeout() == 0.025 and reader.getblocking()
        try:
            stream.readline()
        except socket.timeout as error:
            assert str(error) == 'timed out'
        else:
            raise AssertionError('silent socket did not time out')
        writer.sendall(b'x')
        try:
            stream.read(1)
        except OSError as error:
            assert str(error) == 'cannot read from timed out object'
        else:
            raise AssertionError('timed-out file continued reading')
        assert reader.recv(1) == b'x'
