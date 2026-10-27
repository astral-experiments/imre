"""Open the accepted socket's buffered binary stream for its request handler."""


def _SocketReader(sock):
    return sock.makefile("rb", -1)
