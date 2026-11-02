"""TLS client contexts and blocking, connected sockets using system OpenSSL."""

import _imre_ssl
import _ssl
from _ssl import OPENSSL_VERSION, OPENSSL_VERSION_INFO, OPENSSL_VERSION_NUMBER
from _ssl import nid2obj as _nid2obj, txt2obj as _txt2obj
from collections import namedtuple
from enum import Enum as _Enum, IntEnum as _IntEnum, IntFlag as _IntFlag, _simple_enum

from _ssl import HAS_SNI, HAS_ECDH, HAS_NPN, HAS_ALPN, HAS_PSK, HAS_PHA

CHANNEL_BINDING_TYPES = ["tls-unique"]
HAS_NEVER_CHECK_COMMON_NAME = hasattr(_ssl, "HOSTFLAG_NEVER_CHECK_SUBJECT")
SSL_ERROR_SSL = 1


class TLSVersion(_IntEnum):
    MINIMUM_SUPPORTED = _ssl.PROTO_MINIMUM_SUPPORTED
    SSLv3 = _ssl.PROTO_SSLv3
    TLSv1 = _ssl.PROTO_TLSv1
    TLSv1_1 = _ssl.PROTO_TLSv1_1
    TLSv1_2 = _ssl.PROTO_TLSv1_2
    TLSv1_3 = _ssl.PROTO_TLSv1_3
    MAXIMUM_SUPPORTED = _ssl.PROTO_MAXIMUM_SUPPORTED


for _flag in ("HAS_SSLv2", "HAS_SSLv3", "HAS_TLSv1", "HAS_TLSv1_1", "HAS_TLSv1_2", "HAS_TLSv1_3"):
    if hasattr(_ssl, _flag):
        globals()[_flag] = getattr(_ssl, _flag)
del _flag


class _SSLMethod(_IntEnum):
    if hasattr(_ssl, "PROTOCOL_SSLv3"):
        PROTOCOL_SSLv3 = _ssl.PROTOCOL_SSLv3
    PROTOCOL_TLS = _ssl.PROTOCOL_TLS
    PROTOCOL_TLSv1 = _ssl.PROTOCOL_TLSv1
    PROTOCOL_TLSv1_1 = _ssl.PROTOCOL_TLSv1_1
    PROTOCOL_TLSv1_2 = _ssl.PROTOCOL_TLSv1_2
    PROTOCOL_TLS_CLIENT = _ssl.PROTOCOL_TLS_CLIENT
    PROTOCOL_TLS_SERVER = _ssl.PROTOCOL_TLS_SERVER


def _options_enum():
    # Enum._convert_ sorts by value and name for a consistent canonical alias.
    members = sorted((getattr(_ssl, name), name) for name in dir(_ssl) if name.startswith("OP_"))
    fields = {name: value for value, name in members}
    fields["__module__"] = __name__
    return _simple_enum(_IntFlag)(type("Options", (), fields))


Options = _options_enum()
globals().update(Options.__members__)
del _options_enum


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
if hasattr(_ssl, "PROTOCOL_SSLv3"):
    PROTOCOL_SSLv3 = _SSLMethod.PROTOCOL_SSLv3
PROTOCOL_TLS = _SSLMethod.PROTOCOL_TLS
PROTOCOL_TLSv1 = _SSLMethod.PROTOCOL_TLSv1
PROTOCOL_TLSv1_1 = _SSLMethod.PROTOCOL_TLSv1_1
PROTOCOL_TLSv1_2 = _SSLMethod.PROTOCOL_TLSv1_2
PROTOCOL_TLS_CLIENT = _SSLMethod.PROTOCOL_TLS_CLIENT
PROTOCOL_TLS_SERVER = _SSLMethod.PROTOCOL_TLS_SERVER
PROTOCOL_SSLv23 = _SSLMethod.PROTOCOL_SSLv23 = PROTOCOL_TLS
_PROTOCOL_NAMES = {value: name for name, value in _SSLMethod.__members__.items()}
CERT_NONE = VerifyMode.CERT_NONE
CERT_OPTIONAL = VerifyMode.CERT_OPTIONAL
CERT_REQUIRED = VerifyMode.CERT_REQUIRED


@_simple_enum(_IntEnum)
class _TLSContentType:
    """Content types (record layer)

    See RFC 8446, section B.1
    """
    CHANGE_CIPHER_SPEC = 20
    ALERT = 21
    HANDSHAKE = 22
    APPLICATION_DATA = 23
    # pseudo content types
    HEADER = 0x100
    INNER_CONTENT_TYPE = 0x101


@_simple_enum(_IntEnum)
class _TLSAlertType:
    """Alert types for TLSContentType.ALERT messages

    See RFC 8446, section B.2
    """
    CLOSE_NOTIFY = 0
    UNEXPECTED_MESSAGE = 10
    BAD_RECORD_MAC = 20
    DECRYPTION_FAILED = 21
    RECORD_OVERFLOW = 22
    DECOMPRESSION_FAILURE = 30
    HANDSHAKE_FAILURE = 40
    NO_CERTIFICATE = 41
    BAD_CERTIFICATE = 42
    UNSUPPORTED_CERTIFICATE = 43
    CERTIFICATE_REVOKED = 44
    CERTIFICATE_EXPIRED = 45
    CERTIFICATE_UNKNOWN = 46
    ILLEGAL_PARAMETER = 47
    UNKNOWN_CA = 48
    ACCESS_DENIED = 49
    DECODE_ERROR = 50
    DECRYPT_ERROR = 51
    EXPORT_RESTRICTION = 60
    PROTOCOL_VERSION = 70
    INSUFFICIENT_SECURITY = 71
    INTERNAL_ERROR = 80
    INAPPROPRIATE_FALLBACK = 86
    USER_CANCELED = 90
    NO_RENEGOTIATION = 100
    MISSING_EXTENSION = 109
    UNSUPPORTED_EXTENSION = 110
    CERTIFICATE_UNOBTAINABLE = 111
    UNRECOGNIZED_NAME = 112
    BAD_CERTIFICATE_STATUS_RESPONSE = 113
    BAD_CERTIFICATE_HASH_VALUE = 114
    UNKNOWN_PSK_IDENTITY = 115
    CERTIFICATE_REQUIRED = 116
    NO_APPLICATION_PROTOCOL = 120


@_simple_enum(_IntEnum)
class _TLSMessageType:
    """Message types (handshake protocol)

    See RFC 8446, section B.3
    """
    HELLO_REQUEST = 0
    CLIENT_HELLO = 1
    SERVER_HELLO = 2
    HELLO_VERIFY_REQUEST = 3
    NEWSESSION_TICKET = 4
    END_OF_EARLY_DATA = 5
    HELLO_RETRY_REQUEST = 6
    ENCRYPTED_EXTENSIONS = 8
    CERTIFICATE = 11
    SERVER_KEY_EXCHANGE = 12
    CERTIFICATE_REQUEST = 13
    SERVER_DONE = 14
    CERTIFICATE_VERIFY = 15
    CLIENT_KEY_EXCHANGE = 16
    FINISHED = 20
    CERTIFICATE_URL = 21
    CERTIFICATE_STATUS = 22
    SUPPLEMENTAL_DATA = 23
    KEY_UPDATE = 24
    NEXT_PROTO = 67
    MESSAGE_HASH = 254
    CHANGE_CIPHER_SPEC = 0x0101


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


class _ASN1Object(namedtuple("_ASN1Object", "nid shortname longname oid")):
    """ASN.1 object identifier lookup
    """
    __slots__ = ()

    def __new__(cls, oid):
        return super().__new__(cls, *_txt2obj(oid, name=False))

    @classmethod
    def fromnid(cls, nid):
        """Create _ASN1Object from OpenSSL numeric ID
        """
        return super().__new__(cls, *_nid2obj(nid))

    @classmethod
    def fromname(cls, name):
        """Create _ASN1Object from short name, long name or OID
        """
        return super().__new__(cls, *_txt2obj(name, name=True))


class Purpose(_ASN1Object, _Enum):
    """SSLContext purpose flags with X509v3 Extended Key Usage objects
    """
    def __new__(cls, oid):
        member = _ASN1Object.__new__(cls, oid)
        # Enum reverse lookup uses the typed ASN.1 value as its key.
        member._value_ = _ASN1Object(oid)
        return member

    SERVER_AUTH = '1.3.6.1.5.5.7.3.1'
    CLIENT_AUTH = '1.3.6.1.5.5.7.3.2'


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
    def options(self):
        return Options(_imre_ssl.options(self._ctx))

    @options.setter
    def options(self, value):
        _imre_ssl.options(self._ctx, value)

    @property
    def minimum_version(self):
        return TLSVersion(_imre_ssl.minimum_version(self._ctx))

    @minimum_version.setter
    def minimum_version(self, value):
        if value == TLSVersion.SSLv3:
            if hasattr(_ssl, "OP_ALL"):
                self.options &= ~Options.OP_NO_SSLv3
            else:
                # Inverting a partial IntFlag would also clear unknown higher bits.
                self.options = int(self.options) & ~int(Options.OP_NO_SSLv3)
        _imre_ssl.minimum_version(self._ctx, value)

    @property
    def maximum_version(self):
        return TLSVersion(_imre_ssl.maximum_version(self._ctx))

    @maximum_version.setter
    def maximum_version(self, value):
        _imre_ssl.maximum_version(self._ctx, value)

    @property
    def hostname_checks_common_name(self):
        return _call(_imre_ssl.hostname_checks_common_name, self._ctx)

    @hostname_checks_common_name.setter
    def hostname_checks_common_name(self, enabled):
        _call(_imre_ssl.set_hostname_checks_common_name, self._ctx, enabled)

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

    def get_channel_binding(self, cb_type="tls-unique"):
        if self._session is None:
            if cb_type not in CHANNEL_BINDING_TYPES:
                raise ValueError("{0} channel binding type not implemented".format(cb_type))
            return None
        return _call(_imre_ssl.channel_binding, self._session, cb_type)

    def close(self):
        if self._session is not None:
            _call(_imre_ssl.close, self._session)
            self._session = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        self.close()


def create_default_context(purpose=Purpose.SERVER_AUTH, *, cafile=None, capath=None, cadata=None):
    """Create a verifying client context for server authentication."""
    if not isinstance(purpose, _ASN1Object):
        raise TypeError(purpose)
    if purpose == Purpose.SERVER_AUTH:
        context = SSLContext(PROTOCOL_TLS_CLIENT)
    elif purpose == Purpose.CLIENT_AUTH:
        raise NotImplementedError("server contexts are unsupported")
    else:
        raise ValueError(purpose)
    _call(_imre_ssl.default_verify_flags, context._ctx)
    if cafile or capath or cadata:
        context.load_verify_locations(cafile, capath, cadata)
    else:
        context.set_default_verify_paths()
    return context


_create_default_https_context = create_default_context
