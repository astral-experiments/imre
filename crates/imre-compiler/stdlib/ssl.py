"""TLS client contexts and blocking, connected sockets using system OpenSSL."""

import _imre_ssl
from enum import IntEnum as _IntEnum, IntFlag as _IntFlag

OPENSSL_VERSION = _imre_ssl.OPENSSL_VERSION
SSL_ERROR_SSL = 1


class _SSLMethod(_IntEnum):
    PROTOCOL_TLS_CLIENT = 16


class VerifyMode(_IntEnum):
    CERT_NONE = 0
    CERT_OPTIONAL = 1
    CERT_REQUIRED = 2


# X509_V_FLAG_* values in the OpenSSL 3 public ABI used by the provider.
class VerifyFlags(_IntFlag):
    VERIFY_DEFAULT = 0
    VERIFY_CRL_CHECK_LEAF = 4
    VERIFY_CRL_CHECK_CHAIN = 12
    VERIFY_X509_STRICT = 32
    VERIFY_ALLOW_PROXY_CERTS = 64
    VERIFY_X509_TRUSTED_FIRST = 32768
    VERIFY_X509_PARTIAL_CHAIN = 524288


VERIFY_DEFAULT = VerifyFlags.VERIFY_DEFAULT
VERIFY_CRL_CHECK_LEAF = VerifyFlags.VERIFY_CRL_CHECK_LEAF
VERIFY_CRL_CHECK_CHAIN = VerifyFlags.VERIFY_CRL_CHECK_CHAIN
VERIFY_X509_STRICT = VerifyFlags.VERIFY_X509_STRICT
VERIFY_ALLOW_PROXY_CERTS = VerifyFlags.VERIFY_ALLOW_PROXY_CERTS
VERIFY_X509_TRUSTED_FIRST = VerifyFlags.VERIFY_X509_TRUSTED_FIRST
VERIFY_X509_PARTIAL_CHAIN = VerifyFlags.VERIFY_X509_PARTIAL_CHAIN
PROTOCOL_TLS_CLIENT = _SSLMethod.PROTOCOL_TLS_CLIENT
CERT_NONE = VerifyMode.CERT_NONE
CERT_OPTIONAL = VerifyMode.CERT_OPTIONAL
CERT_REQUIRED = VerifyMode.CERT_REQUIRED


class SSLError(OSError):
    """An error occurred in the SSL implementation."""

    def __str__(self):
        if isinstance(self.strerror, str):
            return self.strerror
        return str(self.args)


class SSLZeroReturnError(SSLError):
    """The peer closed the TLS connection cleanly."""


class SSLWantReadError(SSLError):
    """A read is needed to complete this operation."""


class SSLWantWriteError(SSLError):
    """A write is needed to complete this operation."""


class SSLSyscallError(SSLError):
    """A system call failed during TLS I/O."""


class SSLEOFError(SSLError):
    """The TLS connection terminated abruptly."""


class SSLCertVerificationError(SSLError, ValueError):
    """A certificate could not be verified."""


CertificateError = SSLCertVerificationError


def _call(function, *args):
    try:
        return function(*args)
    except _imre_ssl.Error as exc:
        raise SSLError(SSL_ERROR_SSL, str(exc)) from None


class SSLContext:
    """TLS settings and trusted certificates for client connections."""

    __slots__ = ("_ctx", "_post_handshake_auth")

    def __init__(self, protocol):
        if protocol != PROTOCOL_TLS_CLIENT:
            raise NotImplementedError("only PROTOCOL_TLS_CLIENT is supported")
        self._ctx = _call(_imre_ssl.context)
        self._post_handshake_auth = False

    @property
    def protocol(self):
        return PROTOCOL_TLS_CLIENT

    @property
    def check_hostname(self):
        return True

    @check_hostname.setter
    def check_hostname(self, enabled):
        if not enabled:
            raise NotImplementedError("disabling hostname checks is unsupported")

    @property
    def verify_mode(self):
        return CERT_REQUIRED

    @verify_mode.setter
    def verify_mode(self, mode):
        if mode == CERT_NONE:
            raise ValueError("Cannot set verify_mode to CERT_NONE when check_hostname is enabled.")
        if mode != CERT_REQUIRED:
            raise NotImplementedError("only CERT_REQUIRED is supported")

    @property
    def security_level(self):
        return _call(_imre_ssl.security_level, self._ctx)

    @property
    def verify_flags(self):
        return VerifyFlags(_call(_imre_ssl.verify_flags, self._ctx))

    @property
    def post_handshake_auth(self):
        return self._post_handshake_auth

    @post_handshake_auth.setter
    def post_handshake_auth(self, enabled):
        enabled = bool(enabled)
        _call(_imre_ssl.set_post_handshake_auth, self._ctx, enabled)
        self._post_handshake_auth = enabled

    def set_default_verify_paths(self):
        _call(_imre_ssl.default_verify_paths, self._ctx)

    def set_alpn_protocols(self, alpn_protocols):
        wire = bytearray()
        for protocol in alpn_protocols:
            encoded = bytes(protocol, "ascii")
            if not 1 <= len(encoded) <= 255:
                raise SSLError("ALPN protocols must be 1 to 255 in length")
            wire.append(len(encoded))
            wire.extend(encoded)
        _call(_imre_ssl.set_alpn_protocols, self._ctx, bytes(wire))

    def set_ciphers(self, cipherlist, /):
        _call(_imre_ssl.set_ciphers, self._ctx, cipherlist)

    def load_verify_locations(self, cafile=None, capath=None, cadata=None):
        if capath is not None or cadata is not None:
            raise NotImplementedError("only cafile is supported")
        if cafile is None:
            raise TypeError("cafile, capath and cadata cannot be all omitted")
        _call(_imre_ssl.load_verify_locations, self._ctx, cafile)

    def wrap_socket(self, sock, server_side=False, do_handshake_on_connect=True,
                    suppress_ragged_eofs=True, server_hostname=None, session=None):
        if server_side:
            raise ValueError("Cannot create a server socket with a PROTOCOL_TLS_CLIENT context")
        if not do_handshake_on_connect or not suppress_ragged_eofs or session is not None:
            raise NotImplementedError("only a blocking handshake with default session and EOF handling is supported")
        if not server_hostname:
            raise ValueError("check_hostname requires server_hostname")
        if isinstance(server_hostname, bytes):
            server_hostname = server_hostname.decode("ascii")
        if not isinstance(server_hostname, str):
            raise TypeError("server_hostname must be a string")
        if not server_hostname.isascii():
            raise NotImplementedError("encode internationalized hostnames to IDNA before wrapping the socket")
        result = _call(_imre_ssl.wrap, self._ctx, sock, server_hostname)
        if isinstance(result, tuple):
            code, message = result
            # Match CPython's hostname diagnostics for these X509 verification codes.
            if code == 62:
                message = "Hostname mismatch, certificate is not valid for '%s'." % server_hostname
            elif code == 64:
                message = "IP address mismatch, certificate is not valid for '%s'." % server_hostname
            error = SSLCertVerificationError(
                SSL_ERROR_SSL, "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: " + message
            )
            error.verify_code = code
            error.verify_message = message
            error.library = "SSL"
            error.reason = "CERTIFICATE_VERIFY_FAILED"
            raise error
        peer = object.__new__(SSLSocket)
        peer._session = result
        peer._context = self
        peer._server_hostname = server_hostname
        return peer


class SSLSocket:
    """Encrypted I/O on a socket returned by SSLContext.wrap_socket()."""

    __slots__ = ("_session", "_context", "_server_hostname")

    def __init__(self, *args, **kwargs):
        raise TypeError("SSLSocket does not have a public constructor. Instances are returned by SSLContext.wrap_socket().")

    @property
    def context(self):
        return self._context

    @property
    def server_hostname(self):
        return self._server_hostname

    def sendall(self, data, flags=0):
        if flags:
            raise ValueError("non-zero flags not allowed in calls to sendall() on SSLSocket")
        return _call(_imre_ssl.sendall, self._session, data)

    def recv(self, buflen=1024, flags=0):
        if flags:
            raise ValueError("non-zero flags not allowed in calls to recv() on SSLSocket")
        return _call(_imre_ssl.recv, self._session, buflen)

    def close(self):
        _call(_imre_ssl.close, self._session)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        self.close()


def create_default_context(*, cafile=None, capath=None, cadata=None):
    """Create a verifying client context for server authentication."""
    context = SSLContext(PROTOCOL_TLS_CLIENT)
    _call(_imre_ssl.default_verify_flags, context._ctx)
    if cafile or capath or cadata:
        context.load_verify_locations(cafile, capath, cadata)
    else:
        context.set_default_verify_paths()
    return context


_create_default_https_context = create_default_context
