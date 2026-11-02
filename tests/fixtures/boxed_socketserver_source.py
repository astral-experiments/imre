import socketserver
from socketserver import (
    DatagramRequestHandler,
    StreamRequestHandler,
    ThreadingTCPServer,
    ThreadingUDPServer,
)


def current():
    assert ThreadingUDPServer.__bases__ == (socketserver.ThreadingMixIn, socketserver.UDPServer)
    assert ThreadingTCPServer.__bases__ == (socketserver.ThreadingMixIn, socketserver.TCPServer)
    assert DatagramRequestHandler.__bases__ == (socketserver.BaseRequestHandler,)
    assert StreamRequestHandler.__bases__ == DatagramRequestHandler.__bases__

    sent = []

    class Sink:
        def sendall(self, data):
            sent.append(bytes(data))

    writer = socketserver._SocketWriter(Sink())
    assert writer.write(b"first") == 5
    assert writer.write(bytearray(b"second")) == 6
    writer.close()
    assert sent == [b"first", b"second"]
    print("socketserver imports and writes")
