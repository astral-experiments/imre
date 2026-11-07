import gc
import os
import socket
import ssl
import sys


# Drive both ends in one thread. Neither wrap may wait for or send a ClientHello.
certdata = sys.argv[1]
client_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
client_ctx.load_verify_locations(certdata + "/ssl_cert.pem")
server_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
server_ctx.load_cert_chain(certdata + "/keycert.pem")
with open(certdata + "/ssl_cert.pem") as pem:
    leaf = ssl.PEM_cert_to_DER_cert(pem.read())


class ClientSocket(ssl.SSLSocket):
    forbid_hostname_read = False

    def __getattribute__(self, name):
        if name == "_server_hostname" and self.forbid_hostname_read:
            raise AssertionError("TLS I/O consulted the wrapper's hostname storage")
        return super().__getattribute__(name)

    @property
    def server_hostname(self):
        if self.forbid_hostname_read:
            raise AssertionError("TLS I/O consulted the public hostname getter")
        return self._server_hostname

    @server_hostname.setter
    def server_hostname(self, value):
        self._server_hostname = value


client_ctx.sslsocket_class = ClientSocket


def connect(host, context=client_ctx):
    raw_client, raw_server = socket.socketpair()
    raw_client.setblocking(False)
    raw_server.setblocking(False)
    client = context.wrap_socket(raw_client, server_hostname=host, do_handshake_on_connect=False)
    client.forbid_hostname_read = True
    try:
        raw_server.recv(1)
    except BlockingIOError:
        pass
    else:
        raise AssertionError("deferred wrap sent data")
    server = server_ctx.wrap_socket(raw_server, server_side=True, do_handshake_on_connect=False)
    assert raw_client.fileno() == raw_server.fileno() == -1
    assert not client.getblocking() and client.gettimeout() == 0.0
    return client, server


def want_read(call):
    try:
        call()
    except ssl.SSLWantReadError as exc:
        assert exc.errno == ssl.SSL_ERROR_WANT_READ
    else:
        raise AssertionError("expected a nonblocking TLS read retry")


# First I/O must verify the certificate even if do_handshake() was never called.
client, server = connect("wrong.invalid")
with client, server:
    want_read(lambda: client.send(b"secret"))
    want_read(server.do_handshake)
    try:
        client.send(b"secret")
    except ssl.SSLCertVerificationError as exc:
        assert exc.verify_code == 62 and "wrong.invalid" in exc.verify_message
    else:
        raise AssertionError("unverified application write")
    try:
        server.recv(64)
    except ssl.SSLError as exc:
        assert exc.reason == "SSLV3_ALERT_BAD_CERTIFICATE"
    else:
        raise AssertionError("server received application data")
    # The previous verification result must not turn the next ragged EOF back into a certificate failure.
    assert client.recv(64) == b""

    # An independent TLS connection on this thread can see EOF while the failed session is still live.
    fresh, silent = connect("localhost")
    with fresh, silent:
        silent.shutdown(socket.SHUT_WR)
        try:
            fresh.do_handshake()
        except ssl.SSLEOFError as exc:
            assert exc.errno == ssl.SSL_ERROR_EOF
        else:
            raise AssertionError("handshake succeeded with a peer that closed")

untrusted = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
untrusted.sslsocket_class = ClientSocket
client, server = connect("localhost", untrusted)
with client, server:
    want_read(client.do_handshake)
    want_read(server.do_handshake)
    try:
        client.do_handshake()
    except ssl.SSLCertVerificationError as exc:
        assert exc.verify_code == 18
    else:
        raise AssertionError("accepted an untrusted self-signed certificate")

client, server = connect("localhost")
with client, server:
    try:
        client.getpeercert()
    except ValueError as exc:
        assert str(exc) == "handshake not done yet"
    else:
        raise AssertionError("returned a certificate before the handshake")
    want_read(server.do_handshake)
    want_read(client.do_handshake)
    for _ in range(8):
        done = 0
        for peer in (server, client):
            try:
                peer.do_handshake()
                done += 1
            except ssl.SSLWantReadError:
                pass
        if done == 2:
            break
    else:
        raise AssertionError("TLS handshake did not finish")
    assert client.getpeercert(binary_form=True) == leaf
    assert client.getpeercert()["subject"] and server.getpeercert() is None
    assert client.get_verified_chain()[0] == leaf
    written = client.write(memoryview(b"TLS write"))
    received = server.read(9)
    assert written == 9 and received == b"TLS write"
    assert client.send(b"p\x00ng") == 4
    assert server.recv(64) == b"p\x00ng"
    server.sendall(b"p\xffng")
    assert client.recv(64) == b"p\xffng"
    server.settimeout(0.25)
    assert server.gettimeout() == 0.25 and server.getblocking()

    class ClosePeer:
        def __bool__(self):
            client.close()
            return True

    assert client.getpeercert(ClosePeer()) == leaf
assert client.fileno() == -1 and client.gettimeout() == 0.0
try:
    client.write(b"closed")
except ValueError as exc:
    assert str(exc) == "Write on closed or unwrapped SSL socket."
else:
    raise AssertionError("write accepted a closed TLS socket")
assert server.fileno() == -1 and server.gettimeout() == 0.25

# A failed memory BIO also reports the current operation's EOF.
incoming, outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
peer_in, peer_out = ssl.MemoryBIO(), ssl.MemoryBIO()
class LyingBIO(ssl.SSLObject):
    @property
    def _server_hostname(self):
        raise AssertionError("the configured name belongs to the native session")

    @property
    def server_hostname(self):
        return "unverified.invalid"


client_ctx.sslobject_class = LyingBIO
bio = client_ctx.wrap_bio(incoming, outgoing, server_hostname="wrong.invalid")
# The underlying connection keeps the normalized Python name alive and stable.
configured_name = ssl.SSLObject.server_hostname.__get__
original_id = id(configured_name(bio))
gc.collect()
assert configured_name(bio) == "wrong.invalid" and id(configured_name(bio)) == original_id
assert configured_name(bio) is configured_name(bio)
peer = server_ctx.wrap_bio(peer_in, peer_out, server_side=True)
want_read(bio.do_handshake)
peer_in.write(outgoing.read())
want_read(peer.do_handshake)
incoming.write(peer_out.read())
try:
    bio.do_handshake()
except ssl.SSLCertVerificationError as exc:
    assert exc.verify_code == 62
    assert exc.verify_message == "Hostname mismatch, certificate is not valid for 'wrong.invalid'."
else:
    raise AssertionError("memory BIO accepted a mismatched hostname")
try:
    bio.read(1)
except ssl.SSLEOFError as exc:
    assert exc.errno == ssl.SSL_ERROR_EOF
else:
    raise AssertionError("memory BIO did not report EOF after handshake failure")

# The failed constructor and independently retained streams share the same fd.
class HandshakeError(Exception):
    pass


retained = []
closes = []


class RetainingSocket(ssl.SSLSocket):
    @property
    def _server_hostname(self):
        raise AssertionError("the subclass does not own hostname storage")

    def do_handshake(self, block=False):
        assert self.server_hostname == "localhost"
        retained.extend((self.makefile("rb", 0), self.makefile("wb")))
        raise HandshakeError

    def close(self):
        closes.append(self.fileno())
        super().close()


client_ctx.sslsocket_class = RetainingSocket
raw, other = socket.socketpair()
with raw, other:
    fd = raw.fileno()
    try:
        client_ctx.wrap_socket(raw, server_hostname="localhost")
    except HandshakeError:
        pass
    else:
        raise AssertionError("constructor swallowed the handshake exception")
    assert raw.fileno() == -1 and closes == [fd]
    assert retained[0].fileno() == retained[1].fileno() == fd
    retained[0].close()
    assert retained[1].fileno() == fd and closes == [fd, fd]
    retained[1].close()
    assert closes == [fd, fd, fd]
    try:
        os.fstat(fd)
    except OSError as exc:
        assert exc.errno == 9
    else:
        raise AssertionError("last stream did not release the socket")

print("deferred socket handshake ok")
