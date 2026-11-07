"""TLS contexts, blocking sockets and in-memory handshakes using system OpenSSL."""

import errno as _errno
import _imre_ssl
import _ssl
import os
import socket as _socket
import sys
import warnings as _warnings
from operator import index as _index
from _weakref import ref as _ref
from _ssl import MemoryBIO
from _ssl import OPENSSL_VERSION, OPENSSL_VERSION_INFO, OPENSSL_VERSION_NUMBER
from _ssl import SSLError, SSL_ERROR_SSL as _SSL_ERROR_SSL, nid2obj as _nid2obj, txt2obj as _txt2obj
from collections import namedtuple
from enum import Enum as _Enum, IntEnum as _IntEnum, IntFlag as _IntFlag, _simple_enum

from _ssl import HAS_SNI, HAS_ECDH, HAS_NPN, HAS_ALPN, HAS_PSK, HAS_PHA

CHANNEL_BINDING_TYPES = ["tls-unique"]
HAS_NEVER_CHECK_COMMON_NAME = hasattr(_ssl, "HOSTFLAG_NEVER_CHECK_SUBJECT")


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


def _enum_from_constants(base, name, prefix, exclude=None):
    # Enum._convert_ sorts by value and then name to select the canonical alias.
    members = [(key, value) for key, value in vars(_ssl).items()
               if key.startswith(prefix) and key != exclude]
    try:
        members.sort(key=lambda item: (item[1], item[0]))
    except TypeError:
        members.sort(key=lambda item: item[0])
    fields = dict(members)
    fields["__module__"] = __name__
    cls = _simple_enum(base)(type(name, (), fields))
    globals().update(cls.__members__)
    return cls


_SSLMethod = _enum_from_constants(_IntEnum, "_SSLMethod", "PROTOCOL_", "PROTOCOL_SSLv23")
Options = _enum_from_constants(_IntFlag, "Options", "OP_")
AlertDescription = _enum_from_constants(_IntEnum, "AlertDescription", "ALERT_DESCRIPTION_")
SSLErrorNumber = _enum_from_constants(_IntEnum, "SSLErrorNumber", "SSL_ERROR_")
VerifyFlags = _enum_from_constants(_IntFlag, "VerifyFlags", "VERIFY_")
VerifyMode = _enum_from_constants(_IntEnum, "VerifyMode", "CERT_")
del _enum_from_constants

PROTOCOL_SSLv23 = _SSLMethod.PROTOCOL_SSLv23 = PROTOCOL_TLS
_PROTOCOL_NAMES = {value: name for name, value in _SSLMethod.__members__.items()}


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
        raise SSLError(_SSL_ERROR_SSL, str(exc)) from None


def _check_context_result(result):
    if result is not None:
        code, message, library, reason = result
        error = SSLError(code, message)
        error.library = library
        error.reason = reason
        raise error


def _check_role(protocol, server_side):
    if server_side and protocol == PROTOCOL_TLS_CLIENT:
        raise SSLError(0, "Cannot create a server socket with a PROTOCOL_TLS_CLIENT context")
    if not server_side and protocol == PROTOCOL_TLS_SERVER:
        raise SSLError(0, "Cannot create a client socket with a PROTOCOL_TLS_SERVER context")


def _raise_verify_error(code, message):
    error = SSLCertVerificationError(
        _SSL_ERROR_SSL, "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: " + message
    )
    error.verify_code = code
    error.verify_message = message
    error.library = "SSL"
    error.reason = "CERTIFICATE_VERIFY_FAILED"
    raise error


DefaultVerifyPaths = namedtuple("DefaultVerifyPaths",
    "cafile capath openssl_cafile_env openssl_cafile openssl_capath_env "
    "openssl_capath")

def get_default_verify_paths():
    """Return paths to default cafile and capath.
    """
    parts = _ssl.get_default_verify_paths()

    # environment vars shadow paths
    cafile = os.environ.get(parts[0], parts[1])
    capath = os.environ.get(parts[2], parts[3])

    return DefaultVerifyPaths(cafile if os.path.isfile(cafile) else None,
                              capath if os.path.isdir(capath) else None,
                              *parts)


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
    """TLS settings, trusted certificates and local certificate chains."""

    __slots__ = ("_ctx", "__dict__", "__weakref__")

    def __new__(cls, protocol=None, *args, **kwargs):
        if protocol is None:
            _warnings.warn("ssl.SSLContext() without protocol argument is deprecated.",
                           DeprecationWarning, stacklevel=2)
            protocol = PROTOCOL_TLS
        if not issubclass(cls, __class__):
            raise TypeError("expected an SSLContext subclass")
        protocol = _index(protocol)
        if protocol == PROTOCOL_TLS_CLIENT:
            ctx = _call(_imre_ssl.context)
        elif protocol == PROTOCOL_TLS_SERVER:
            ctx = _call(_imre_ssl.server_context)
        elif protocol == PROTOCOL_TLS:
            _warnings.warn("ssl.PROTOCOL_TLS is deprecated", DeprecationWarning, stacklevel=2)
            ctx = _call(_imre_ssl.generic_context)
        elif protocol in _PROTOCOL_NAMES:
            ctx = _call(_imre_ssl.context, protocol)
        elif not -(1 << 31) <= protocol < (1 << 31):
            raise OverflowError("Python int too large to convert to C int")
        else:
            raise ValueError("invalid or unsupported protocol version %i" % protocol)
        self = object.__new__(cls)
        __class__._ctx.__set__(self, ctx)
        return self

    @property
    def protocol(self):
        return _SSLMethod(_imre_ssl.protocol(__class__._ctx.__get__(self)))

    @property
    def check_hostname(self):
        return _imre_ssl.check_hostname(__class__._ctx.__get__(self))

    @check_hostname.setter
    def check_hostname(self, enabled):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("check_hostname", self)
        _imre_ssl.check_hostname(__class__._ctx.__get__(self), enabled)

    @property
    def verify_mode(self):
        return VerifyMode(_imre_ssl.verify_mode(__class__._ctx.__get__(self)))

    @verify_mode.setter
    def verify_mode(self, mode):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("verify_mode", self)
        _imre_ssl.verify_mode(__class__._ctx.__get__(self), mode)

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
    def num_tickets(self):
        """Control the number of TLSv1.3 session tickets."""
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("num_tickets", self)
        return _imre_ssl.num_tickets(__class__._ctx.__get__(self))

    @num_tickets.setter
    def num_tickets(self, value):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("num_tickets", self)
        _imre_ssl.num_tickets(__class__._ctx.__get__(self), value)

    @property
    def security_level(self):
        return _call(_imre_ssl.security_level, self._ctx)

    @property
    def verify_flags(self):
        return VerifyFlags(_call(_imre_ssl.verify_flags, __class__._ctx.__get__(self)))

    @verify_flags.setter
    def verify_flags(self, value):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("verify_flags", self)
        _call(_imre_ssl.verify_flags, __class__._ctx.__get__(self), value)

    @property
    def post_handshake_auth(self):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("post_handshake_auth", self)
        return _imre_ssl.post_handshake_auth(__class__._ctx.__get__(self))

    @post_handshake_auth.setter
    def post_handshake_auth(self, enabled):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("post_handshake_auth", self)
        _imre_ssl.post_handshake_auth(__class__._ctx.__get__(self), enabled)

    def session_stats(self, /):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("session_stats", self)
        return _imre_ssl.session_stats(__class__._ctx.__get__(self))

    def cert_store_stats(self, /):
        if not issubclass(type(self), SSLContext):
            _imre_ssl.raise_context_receiver_error("cert_store_stats", self)
        return _imre_ssl.cert_store_stats(self._ctx)

    def get_ca_certs(self, /, binary_form=False):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("get_ca_certs", self)
        return _call(_imre_ssl.get_ca_certs, __class__._ctx.__get__(self), binary_form)

    def set_default_verify_paths(self):
        _call(_imre_ssl.default_verify_paths, self._ctx)

    def load_default_certs(self, purpose=Purpose.SERVER_AUTH):
        if not isinstance(purpose, _ASN1Object):
            raise TypeError(purpose)
        # The native provider runs on POSIX and resolves its trust paths itself.
        self.set_default_verify_paths()

    def set_alpn_protocols(self, alpn_protocols):
        wire = bytearray()
        for protocol in alpn_protocols:
            encoded = bytes(protocol, "ascii")
            if not 1 <= len(encoded) <= 255:
                raise SSLError("ALPN protocols must be 1 to 255 in length")
            wire.append(len(encoded))
            wire.extend(encoded)
        _call(_imre_ssl.set_alpn_protocols, self._ctx, bytes(wire))

    def get_ciphers(self, /):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("get_ciphers", self)
        return _call(_imre_ssl.get_ciphers, __class__._ctx.__get__(self))

    def set_ecdh_curve(self, name, /):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("set_ecdh_curve", self)
        result = _imre_ssl.set_ecdh_curve(__class__._ctx.__get__(self), name)
        if result is False:
            raise ValueError(f"unknown elliptic curve name {name!r}")
        _check_context_result(result)

    def set_ciphers(self, cipherlist, /):
        _call(_imre_ssl.set_ciphers, self._ctx, cipherlist)

    def load_dh_params(self, path, /):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("load_dh_params", self)
        result = _call(_imre_ssl.load_dh_params, __class__._ctx.__get__(self), path)
        _check_context_result(result)

    def load_cert_chain(self, certfile, keyfile=None, password=None):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("load_cert_chain", self)
        result = _call(_imre_ssl.load_cert_chain, __class__._ctx.__get__(self),
                       certfile, keyfile, password)
        _check_context_result(result)

    def load_verify_locations(self, /, cafile=None, capath=None, cadata=None):
        if not issubclass(type(self), __class__):
            _imre_ssl.raise_context_receiver_error("load_verify_locations", self)
        result = _call(_imre_ssl.load_verify_locations, __class__._ctx.__get__(self),
                       cafile, capath, cadata)
        _check_context_result(result)

    def _encode_hostname(self, hostname):
        if hostname is None:
            return None
        if isinstance(hostname, str):
            return hostname.encode("idna").decode("ascii")
        return hostname.decode("ascii")

    @property
    def sni_callback(self):
        return _call(_imre_ssl.sni_callback, __class__._ctx.__get__(self))

    @sni_callback.setter
    def sni_callback(self, callback):
        _call(_imre_ssl.sni_callback, __class__._ctx.__get__(self), callback)

    @sni_callback.deleter
    def sni_callback(self):
        _call(_imre_ssl.sni_callback, __class__._ctx.__get__(self), None)
        raise TypeError("not a callable object")

    def set_servername_callback(self, server_name_callback):
        if server_name_callback is None:
            self.sni_callback = None
        else:
            if not callable(server_name_callback):
                raise TypeError("not a callable object")

            def shim_cb(sslobj, servername, sslctx):
                return server_name_callback(sslobj, self._encode_hostname(servername), sslctx)

            self.sni_callback = shim_cb

    def wrap_bio(self, incoming, outgoing, server_side=False, server_hostname=None, session=None):
        return self.sslobject_class._create(incoming, outgoing, server_side=server_side,
                                            server_hostname=self._encode_hostname(server_hostname),
                                            session=session, context=self)

    def wrap_socket(self, sock, server_side=False, do_handshake_on_connect=True,
                    suppress_ragged_eofs=True, server_hostname=None, session=None):
        return self.sslsocket_class._create(
            sock=sock, server_side=server_side, do_handshake_on_connect=do_handshake_on_connect,
            suppress_ragged_eofs=suppress_ragged_eofs, server_hostname=server_hostname,
            context=self, session=session,
        )


_TLS_ERRORS = {
    "WANT_READ": (SSLWantReadError, _ssl.SSL_ERROR_WANT_READ, "The operation did not complete (read)", None),
    "WANT_WRITE": (SSLWantWriteError, _ssl.SSL_ERROR_WANT_WRITE, "The operation did not complete (write)", None),
    "ZERO_RETURN": (SSLZeroReturnError, _ssl.SSL_ERROR_ZERO_RETURN, "TLS/SSL connection has been closed (EOF)", None),
    "EOF": (SSLEOFError, _ssl.SSL_ERROR_EOF, "EOF occurred in violation of protocol", None),
    "UNEXPECTED_EOF_WHILE_READING": (SSLEOFError, _ssl.SSL_ERROR_EOF, "EOF occurred in violation of protocol", "SSL"),
    "PROTOCOL_IS_SHUTDOWN": (SSLError, _SSL_ERROR_SSL, "protocol is shutdown", "SSL"),
    "SHUTDOWN_WHILE_IN_INIT": (SSLError, _SSL_ERROR_SSL, "shutdown while in init", "SSL"),
}


def _tls_result(result):
    if isinstance(result, tuple):
        # Certificate errors carry a numeric verification code; protocol errors carry a reason name.
        reason, message = result
        if isinstance(reason, int):
            _raise_verify_error(reason, message)
        kind, code, library = SSLError, _SSL_ERROR_SSL, "SSL"
    elif isinstance(result, str):
        reason = result
        kind, code, message, library = _TLS_ERRORS[reason]
    else:
        return result
    if library:
        prefix = f"{library}: {reason}" if reason else library
        message = f"[{prefix}] {message}"
    error = kind(code, message)
    error.library = library
    error.reason = reason if library else None
    raise error


def _set_context(session, context):
    if not issubclass(type(context), SSLContext):
        raise TypeError("The value must be a SSLContext")
    _tls_result(_call(_imre_ssl.set_context, session, SSLContext._ctx.__get__(context), context))


def _bio_handshake(session):
    return _tls_result(_call(_imre_ssl.bio_handshake, session))


class SSLObject:
    """A TLS connection driven by caller-owned incoming and outgoing memory BIOs."""

    def __init__(self, *args, **kwargs):
        raise TypeError(f"{self.__class__.__name__} does not have a public constructor. "
                        "Instances are returned by SSLContext.wrap_bio().")

    @classmethod
    def _create(cls, incoming, outgoing, server_side=False, server_hostname=None,
                session=None, context=None):
        server_side = bool(server_side)
        native_context = SSLContext._ctx.__get__(context)
        _check_role(_imre_ssl.protocol(native_context), server_side)
        if session is not None:
            raise NotImplementedError("setting a session on a memory BIO is unsupported")
        self = cls.__new__(cls)
        self._sslobj = _call(_imre_ssl.wrap_bio, native_context, incoming, outgoing,
                             server_hostname, server_side, context, _ref(self))
        self._context = context
        self._server_side = server_side
        return self

    @property
    def context(self):
        return self._context

    @context.setter
    def context(self, context):
        _set_context(self._sslobj, context)
        self._context = context

    @property
    def server_side(self):
        return self._server_side

    @property
    def server_hostname(self):
        return _imre_ssl.bio_server_hostname(self._sslobj)

    def _bio_call(self, operation, *args):
        return _tls_result(_call(operation, self._sslobj, *args))

    def read(self, len=1024, buffer=None):
        """Read up to len decrypted bytes, or return the number read into buffer."""
        return self._bio_call(_imre_ssl.bio_read, len, buffer)

    def write(self, data):
        """Write bytes-like data and return the number of plaintext bytes accepted."""
        return self._bio_call(_imre_ssl.bio_write, data)

    def pending(self):
        """Return the number of already decrypted bytes available to read."""
        return self._bio_call(_imre_ssl.bio_pending)

    def getpeercert(self, binary_form=False):
        """Return the peer certificate as a dictionary or DER bytes, or None if absent."""
        return _call(_imre_ssl.bio_getpeercert, self._sslobj, binary_form)

    def do_handshake(self):
        """Advance the handshake, raising SSLWantReadError when more peer data is needed."""
        self._sslobj.do_handshake()

    def selected_alpn_protocol(self):
        return _imre_ssl.selected_alpn_protocol(self._sslobj)

    def verify_client_post_handshake(self):
        _check_context_result(_imre_ssl.verify_client_post_handshake(self._sslobj))

    def unwrap(self):
        """Send and receive TLS close notifications through the memory BIOs."""
        self._bio_call(_imre_ssl.bio_shutdown)


SSLContext.sslobject_class = SSLObject


class SSLSocket(_socket.socket):
    """Encrypted I/O on a socket returned by SSLContext.wrap_socket()."""

    __slots__ = ("_session", "_context", "_server_hostname", "server_side",
                 "do_handshake_on_connect", "suppress_ragged_eofs", "_connected")

    def __init__(self, *args, **kwargs):
        raise TypeError("SSLSocket does not have a public constructor. Instances are returned by SSLContext.wrap_socket().")

    @classmethod
    def _create(cls, sock, server_side=False, do_handshake_on_connect=True,
                suppress_ragged_eofs=True, server_hostname=None, context=None, session=None):
        if sock.getsockopt(_socket.SOL_SOCKET, _socket.SO_TYPE) != _socket.SOCK_STREAM:
            raise NotImplementedError("only stream sockets are supported")
        if server_side and server_hostname is not None:
            raise ValueError("server_hostname can only be specified in client mode")
        if server_side and session is not None:
            raise ValueError("session can only be specified in client mode")
        if context.check_hostname and not server_hostname:
            raise ValueError("check_hostname requires server_hostname")
        _check_role(context.protocol, server_side)
        if session is not None:
            raise NotImplementedError("only the default session is supported")
        if not server_side and server_hostname is not None:
            if isinstance(server_hostname, bytes):
                server_hostname = server_hostname.decode("ascii")
            if not isinstance(server_hostname, str):
                raise TypeError("server_hostname must be a string")
            if not server_hostname.isascii():
                raise NotImplementedError("encode internationalized hostnames to IDNA before wrapping the socket")
        sock_timeout = sock.gettimeout()
        args = dict(family=sock.family, type=sock.type, proto=sock.proto, fileno=sock.fileno())
        peer = cls.__new__(cls, **args)
        if not issubclass(type(peer), SSLSocket):
            raise TypeError("SSLSocket.__new__ must return an SSLSocket instance")
        super(SSLSocket, peer).__init__(**args)
        sock.detach()
        try:
            SSLSocket._session.__set__(peer, None)
            peer._context = context
            SSLSocket._server_hostname.__set__(peer, server_hostname)
            peer.server_side = bool(server_side)
            peer.do_handshake_on_connect = do_handshake_on_connect
            peer.suppress_ragged_eofs = suppress_ragged_eofs
            try:
                peer.getpeername()
            except OSError as exc:
                if exc.errno != _errno.ENOTCONN or not server_side:
                    raise
                connected = False
                # Reject cleartext queued before wrapping a disconnected server endpoint.
                blocking = peer.getblocking()
                peer.setblocking(False)
                try:
                    pending = peer.recv(1)
                except OSError as read_error:
                    if read_error.errno not in (_errno.ENOTCONN, _errno.EINVAL):
                        raise
                    pending = b""
                peer.setblocking(blocking)
                if pending:
                    reason = "Closed before TLS handshake with data in recv buffer."
                    error = SSLError(_errno.ENOTCONN, reason)
                    error.reason = reason
                    error.library = None
                    raise error
            else:
                connected = True
            peer.settimeout(sock_timeout)
            result = _call(_imre_ssl.wrap, context._ctx, peer, server_hostname,
                           bool(server_side), connected, context, _ref(peer))
            result = _tls_result(result)
            SSLSocket._session.__set__(peer, result)
            connected = _imre_ssl.is_session(result)
            peer._connected = connected
            if connected and do_handshake_on_connect:
                if peer.gettimeout() == 0.0:
                    raise ValueError("do_handshake_on_connect should not be specified for non-blocking sockets")
                peer.do_handshake()
        except BaseException:
            try:
                peer.close()
            except OSError:
                pass
            raise
        return peer

    @property
    def context(self):
        return self._context

    @context.setter
    def context(self, context):
        self._context = context
        if not _imre_ssl.is_session(self._session):
            # The public context changes even when there is no underlying SSL object.
            setattr(None, "context", context)
        else:
            _set_context(self._session, context)

    @property
    def server_hostname(self):
        return SSLSocket._server_hostname.__get__(self)

    def do_handshake(self, block=False):
        if not _imre_ssl.is_session(self._session):
            self.getpeername()
            raise AttributeError("'NoneType' object has no attribute 'do_handshake'")
        timeout = self.gettimeout()
        try:
            if timeout == 0.0 and block:
                self.settimeout(None)
            return _tls_result(_call(_imre_ssl.do_handshake, self._session))
        finally:
            self.settimeout(timeout)

    def connect(self, addr):
        if self.server_side:
            raise ValueError("can't connect in server-side mode")
        # Client sockets admitted by _create are already connected and wrapped.
        raise ValueError("attempt to connect already-connected SSLSocket!")

    def connect_ex(self, addr):
        return SSLSocket.connect(self, addr)

    def accept(self):
        child, address = super().accept()
        try:
            peer = self.context.wrap_socket(
                child, server_side=True,
                do_handshake_on_connect=self.do_handshake_on_connect,
                suppress_ragged_eofs=self.suppress_ragged_eofs,
            )
        except BaseException:
            child.close()
            raise
        return peer, address

    def _checkClosed(self, msg=None):
        pass

    def _check_connected(self):
        if not self._connected:
            self.getpeername()

    def getpeercert(self, binary_form=False):
        """Return the peer certificate as a dictionary or DER bytes, or None."""
        self._checkClosed()
        self._check_connected()
        return _call(_imre_ssl.getpeercert, self._session, binary_form)

    def get_verified_chain(self):
        """Return the verified certificate chain as a list of DER-encoded certificates."""
        return _call(_imre_ssl.get_verified_chain, self._session)

    def get_unverified_chain(self):
        """Return the certificate chain sent by the peer as a list of DER-encoded certificates."""
        return _call(_imre_ssl.get_unverified_chain, self._session)

    def send(self, data, flags=0):
        if self._session is None:
            return super().send(data, flags)
        if _imre_ssl.is_session(self._session) and flags != 0:
            raise ValueError("non-zero flags not allowed in calls to send() on %s" % self.__class__)
        return _tls_result(_call(_imre_ssl.send, self._session, data, flags))

    def sendall(self, data, flags=0):
        if self._session is None:
            return super().sendall(data, flags)
        if _imre_ssl.is_session(self._session) and flags:
            raise ValueError("non-zero flags not allowed in calls to sendall() on SSLSocket")
        return _tls_result(_call(_imre_ssl.sendall, self._session, data, flags))

    def read(self, len=1024, buffer=None):
        if not _imre_ssl.is_session(self._session):
            raise ValueError("Read on closed or unwrapped SSL socket.")
        try:
            return _tls_result(_call(_imre_ssl.read, self._session, len, buffer))
        except SSLError as exc:
            if exc.args[0] == SSL_ERROR_EOF and self.suppress_ragged_eofs:
                return b"" if buffer is None else 0
            raise

    def recv(self, buflen=1024, flags=0):
        if self._session is None:
            return super().recv(buflen, flags)
        if _imre_ssl.is_session(self._session):
            if flags:
                raise ValueError("non-zero flags not allowed in calls to recv() on SSLSocket")
            return self.read(buflen)
        return _call(_imre_ssl.recv, self._session, buflen, flags)

    def recv_into(self, buffer, nbytes=None, flags=0):
        if nbytes is None:
            nbytes = memoryview(buffer).nbytes or 1024
        if self._session is None:
            return super().recv_into(buffer, nbytes, flags)
        if flags:
            raise ValueError("non-zero flags not allowed in calls to recv_into() on %s" % self.__class__)
        return self.read(nbytes, buffer)

    def dup(self):
        raise NotImplementedError("Can't dup() %s instances" % self.__class__.__name__)

    def sendto(self, data, *args):
        if self._session is not None:
            raise ValueError("sendto not allowed on instances of %s" % self.__class__)
        return super().sendto(data, *args)

    def recvfrom(self, *args):
        if self._session is not None:
            raise ValueError("recvfrom not allowed on instances of %s" % self.__class__)
        return super().recvfrom(*args)

    def recvfrom_into(self, *args):
        if self._session is not None:
            raise ValueError("recvfrom_into not allowed on instances of %s" % self.__class__)
        return super().recvfrom_into(*args)

    def sendmsg(self, *args, **kwargs):
        raise NotImplementedError("sendmsg not allowed on instances of %s" % self.__class__)

    def recvmsg(self, *args, **kwargs):
        raise NotImplementedError("recvmsg not allowed on instances of %s" % self.__class__)

    def recvmsg_into(self, *args, **kwargs):
        raise NotImplementedError("recvmsg_into not allowed on instances of %s" % self.__class__)

    def pending(self):
        if not _imre_ssl.is_session(self._session):
            return 0
        return _tls_result(_call(_imre_ssl.pending, self._session))

    def selected_alpn_protocol(self):
        return _imre_ssl.selected_alpn_protocol(self._session)

    def verify_client_post_handshake(self):
        if not _imre_ssl.is_session(self._session):
            raise ValueError("No SSL wrapper around " + str(self))
        _check_context_result(_imre_ssl.verify_client_post_handshake(self._session))

    def unwrap(self):
        if not _imre_ssl.is_session(self._session):
            raise ValueError("No SSL wrapper around " + str(self))
        _tls_result(_call(_imre_ssl.tls_shutdown, self._session))
        self._session = _imre_ssl.release_session(self._session)
        return self

    def get_channel_binding(self, cb_type="tls-unique"):
        if not _imre_ssl.is_session(self._session):
            if cb_type not in CHANNEL_BINDING_TYPES:
                raise ValueError("{0} channel binding type not implemented".format(cb_type))
            return None
        return _call(_imre_ssl.channel_binding, self._session, cb_type)

    def shutdown(self, how):
        self._session = _imre_ssl.release_session(self._session)
        super().shutdown(how)

    def _real_close(self):
        _imre_ssl.release_session(getattr(self, "_session", None))
        SSLSocket._session.__set__(self, None)
        super()._real_close()


SSLContext.sslsocket_class = SSLSocket


def create_default_context(purpose=Purpose.SERVER_AUTH, *, cafile=None, capath=None, cadata=None):
    """Create a context with the default settings for the requested purpose."""
    if not isinstance(purpose, _ASN1Object):
        raise TypeError(purpose)
    if purpose == Purpose.SERVER_AUTH:
        context = SSLContext(PROTOCOL_TLS_CLIENT)
    elif purpose == Purpose.CLIENT_AUTH:
        context = SSLContext(PROTOCOL_TLS_SERVER)
    else:
        raise ValueError(purpose)
    _call(_imre_ssl.default_verify_flags, context._ctx)
    if cafile or capath or cadata:
        context.load_verify_locations(cafile, capath, cadata)
    elif context.verify_mode != CERT_NONE:
        context.load_default_certs(purpose)
    return context


def _create_unverified_context(protocol=None, *, cert_reqs=CERT_NONE,
                           check_hostname=False, purpose=Purpose.SERVER_AUTH,
                           certfile=None, keyfile=None,
                           cafile=None, capath=None, cadata=None):
    """Create a SSLContext object for Python stdlib modules

    All Python stdlib modules shall use this function to create SSLContext
    objects in order to keep common settings in one place. The configuration
    is less restrict than create_default_context()'s to increase backward
    compatibility.
    """
    if not isinstance(purpose, _ASN1Object):
        raise TypeError(purpose)

    # SSLContext sets OP_NO_SSLv2, OP_NO_SSLv3, OP_NO_COMPRESSION,
    # OP_CIPHER_SERVER_PREFERENCE, OP_SINGLE_DH_USE and OP_SINGLE_ECDH_USE
    # by default.
    if purpose == Purpose.SERVER_AUTH:
        # verify certs and host name in client mode
        if protocol is None:
            protocol = PROTOCOL_TLS_CLIENT
    elif purpose == Purpose.CLIENT_AUTH:
        if protocol is None:
            protocol = PROTOCOL_TLS_SERVER
    else:
        raise ValueError(purpose)

    context = SSLContext(protocol)
    context.check_hostname = check_hostname
    if cert_reqs is not None:
        context.verify_mode = cert_reqs
    if check_hostname:
        context.check_hostname = True

    if keyfile and not certfile:
        raise ValueError("certfile must be specified")
    if certfile or keyfile:
        context.load_cert_chain(certfile, keyfile)

    # load CA root certs
    if cafile or capath or cadata:
        context.load_verify_locations(cafile, capath, cadata)
    elif context.verify_mode != CERT_NONE:
        # no explicit cafile, capath or cadata but the verify mode is
        # CERT_OPTIONAL or CERT_REQUIRED. Let's try to load default system
        # root CA certificates for the given purpose. This may fail silently.
        context.load_default_certs(purpose)
    # OpenSSL 1.1.1 keylog file
    if hasattr(context, 'keylog_filename'):
        keylogfile = os.environ.get('SSLKEYLOGFILE')
        if keylogfile and not sys.flags.ignore_environment:
            context.keylog_filename = keylogfile
    return context


_create_default_https_context = create_default_context
_create_stdlib_context = _create_unverified_context


def cert_time_to_seconds(cert_time):
    """Return the time in seconds since the Epoch, given the timestring
    representing the "notBefore" or "notAfter" date from a certificate
    in ``"%b %d %H:%M:%S %Y %Z"`` strptime format (C locale).

    "notBefore" or "notAfter" dates must use UTC (RFC 5280).

    Month is one of: Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec
    UTC should be specified as GMT (see ASN1_TIME_print())
    """
    from _strptime import _strptime
    from calendar import timegm

    months = (
        "Jan","Feb","Mar","Apr","May","Jun",
        "Jul","Aug","Sep","Oct","Nov","Dec"
    )
    time_format = ' %d %H:%M:%S %Y GMT' # NOTE: no month, fixed GMT
    try:
        month = cert_time[:3]
        # Titlecasing a plain binary value cannot match a text month.
        month_type = type(month)
        if month_type is not bytes and month_type is not bytearray:
            month = month.title()
        month_number = months.index(month) + 1
    except ValueError:
        raise ValueError('time data %r does not match '
                         'format "%%b%s"' % (cert_time, time_format))
    else:
        # found valid month
        # _strptime_time wraps these same fields in struct_time.
        tt = _strptime(cert_time[3:], time_format)[0]
        # return an integer, the previous mktime()-based implementation
        # returned a float (fractional seconds are always zero here).
        return timegm((tt[0], month_number) + tt[2:6])


PEM_HEADER = "-----BEGIN CERTIFICATE-----"
PEM_FOOTER = "-----END CERTIFICATE-----"

def DER_cert_to_PEM_cert(der_cert_bytes):
    """Takes a certificate in binary DER format and returns the
    PEM version of it as a string."""

    import base64

    f = str(base64.standard_b64encode(der_cert_bytes), 'ASCII', 'strict')
    ss = [PEM_HEADER]
    ss += [f[i:i+64] for i in range(0, len(f), 64)]
    ss.append(PEM_FOOTER + '\n')
    return '\n'.join(ss)

def PEM_cert_to_DER_cert(pem_cert_string):
    """Takes a certificate in ASCII PEM format and returns the
    DER-encoded version of it as a byte sequence"""

    import base64

    if not pem_cert_string.startswith(PEM_HEADER):
        raise ValueError("Invalid PEM encoding; must start with %s"
                         % PEM_HEADER)
    if not pem_cert_string.strip().endswith(PEM_FOOTER):
        raise ValueError("Invalid PEM encoding; must end with %s"
                         % PEM_FOOTER)
    d = pem_cert_string.strip()[len(PEM_HEADER):-len(PEM_FOOTER)]
    return base64.decodebytes(d.encode('ASCII', 'strict'))
