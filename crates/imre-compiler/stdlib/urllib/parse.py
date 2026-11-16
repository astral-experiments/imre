# URL helpers from CPython 3.14.5 Lib/urllib/parse.py (PSF-2.0).

from collections import namedtuple
import functools
import math
import re
import warnings

__all__ = ["urlparse", "urlunparse", "urljoin", "urldefrag",
           "urlsplit", "urlunsplit", "urlencode", "parse_qs",
           "parse_qsl", "quote", "quote_plus", "quote_from_bytes",
           "unquote", "unquote_plus", "unquote_to_bytes",
           "DefragResult", "ParseResult", "SplitResult",
           "DefragResultBytes", "ParseResultBytes", "SplitResultBytes"]


@functools.lru_cache(typed=True)
def urlsplit(url, scheme='', allow_fragments=True):
    """Parse a URL into 5 components:
    <scheme>://<netloc>/<path>?<query>#<fragment>

    The result is a named 5-tuple with fields corresponding to the
    above. It is either a SplitResult or SplitResultBytes object,
    depending on the type of the url parameter.

    The username, password, hostname, and port sub-components of netloc
    can also be accessed as attributes of the returned object.

    The scheme argument provides the default value of the scheme
    component when no scheme is found in url.

    If allow_fragments is False, no attempt is made to separate the
    fragment component from the previous component, which can be either
    path or query.

    Note that % escapes are not expanded.
    """
    fields = _imre_urlsplit(url, scheme, allow_fragments)
    result = SplitResult(*fields)
    return result if isinstance(url, str) else result.encode('ascii', 'strict')


def urlparse(url, scheme='', allow_fragments=True):
    """Parse a URL into 6 components:
    <scheme>://<netloc>/<path>;<params>?<query>#<fragment>

    The result is a named 6-tuple with fields corresponding to the
    above. It is either a ParseResult or ParseResultBytes object,
    depending on the type of the url parameter.

    The username, password, hostname, and port sub-components of netloc
    can also be accessed as attributes of the returned object.

    The scheme argument provides the default value of the scheme
    component when no scheme is found in url.

    If allow_fragments is False, no attempt is made to separate the
    fragment component from the previous component, which can be either
    path or query.

    Note that % escapes are not expanded.

    urlsplit() should generally be used instead of urlparse().
    """
    fields = _imre_urlparse(url, scheme, allow_fragments)
    result = ParseResult(*fields)
    return result if isinstance(url, str) else result.encode('ascii', 'strict')


class _ResultMixinStr(object):
    """Standard approach to encoding parsed results from str to bytes"""
    __slots__ = ()

    def encode(self, encoding='ascii', errors='strict'):
        return self._encoded_counterpart(*(x.encode(encoding, errors) for x in self))


class _ResultMixinBytes(object):
    """Standard approach to decoding parsed results from bytes to str"""
    __slots__ = ()

    def decode(self, encoding='ascii', errors='strict'):
        return self._decoded_counterpart(*(x.decode(encoding, errors) for x in self))


_DefragResultBase = namedtuple('_DefragResultBase', 'url fragment')


_DefragResultBase.__doc__ = """
DefragResult(url, fragment)

A 2-tuple that contains the url without fragment identifier and the fragment
identifier as a separate argument.
"""


_DefragResultBase.url.__doc__ = """The URL with no fragment identifier."""


_DefragResultBase.fragment.__doc__ = """
Fragment identifier separated from URL, that allows indirect identification of a
secondary resource by reference to a primary resource and additional identifying
information.
"""


class DefragResult(_DefragResultBase, _ResultMixinStr):
    __slots__ = ()
    def geturl(self):
        if self.fragment:
            return self.url + '#' + self.fragment
        else:
            return self.url


class DefragResultBytes(_DefragResultBase, _ResultMixinBytes):
    __slots__ = ()
    def geturl(self):
        if self.fragment:
            return self.url + b'#' + self.fragment
        else:
            return self.url


DefragResult._encoded_counterpart = DefragResultBytes
DefragResultBytes._decoded_counterpart = DefragResult


_SplitResultBase = namedtuple('_SplitResultBase', 'scheme netloc path query fragment')
_ParseResultBase = namedtuple('_ParseResultBase', 'scheme netloc path params query fragment')


class _NetlocResultMixinBase:
    __slots__ = ()

    @property
    def username(self):
        return _netloc_property(self.netloc, 'username')

    @property
    def password(self):
        return _netloc_property(self.netloc, 'password')

    @property
    def hostname(self):
        return _netloc_property(self.netloc, 'hostname')

    @property
    def port(self):
        return _netloc_property(self.netloc, 'port')


class SplitResult(_SplitResultBase, _NetlocResultMixinBase, _ResultMixinStr):
    __slots__ = ()

    def geturl(self):
        return urlunsplit(self)


class ParseResult(_ParseResultBase, _NetlocResultMixinBase, _ResultMixinStr):
    __slots__ = ()

    def geturl(self):
        return urlunparse(self)


class SplitResultBytes(_SplitResultBase, _NetlocResultMixinBase, _ResultMixinBytes):
    __slots__ = ()

    def geturl(self):
        return urlunsplit(self)


class ParseResultBytes(_ParseResultBase, _NetlocResultMixinBase, _ResultMixinBytes):
    __slots__ = ()

    def geturl(self):
        return urlunparse(self)


SplitResult._encoded_counterpart = SplitResultBytes
SplitResultBytes._decoded_counterpart = SplitResult
ParseResult._encoded_counterpart = ParseResultBytes
ParseResultBytes._decoded_counterpart = ParseResult


_WHATWG_C0_CONTROL_OR_SPACE = '\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c\r\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f '


def splittype(url):
    warnings.warn("urllib.parse.splittype() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splittype(url)


_typeprog = None
def _splittype(url):
    """splittype('type:opaquestring') --> 'type', 'opaquestring'."""
    global _typeprog
    if _typeprog is None:
        _typeprog = re.compile('([^/:]+):(.*)', re.DOTALL)

    match = _typeprog.match(url)
    if match:
        scheme, data = match.groups()
        return scheme.lower(), data
    return None, url


def splithost(url):
    warnings.warn("urllib.parse.splithost() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splithost(url)


_hostprog = None
def _splithost(url):
    """splithost('//host[:port]/path') --> 'host[:port]', '/path'."""
    global _hostprog
    if _hostprog is None:
        _hostprog = re.compile('//([^/#?]*)(.*)', re.DOTALL)

    match = _hostprog.match(url)
    if match:
        host_port, path = match.groups()
        if path and path[0] != '/':
            path = '/' + path
        return host_port, path
    return None, url


def splituser(host):
    warnings.warn("urllib.parse.splituser() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splituser(host)


def _splituser(host):
    """splituser('user[:passwd]@host[:port]') --> 'user[:passwd]', 'host[:port]'."""
    user, delim, host = host.rpartition('@')
    return (user if delim else None), host


def splitpasswd(user):
    warnings.warn("urllib.parse.splitpasswd() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitpasswd(user)


def _splitpasswd(user):
    """splitpasswd('user:passwd') -> 'user', 'passwd'."""
    user, delim, passwd = user.partition(':')
    return user, (passwd if delim else None)


def splitport(host):
    warnings.warn("urllib.parse.splitport() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitport(host)


_portprog = None
def _splitport(host):
    """splitport('host:port') --> 'host', 'port'."""
    global _portprog
    if _portprog is None:
        _portprog = re.compile('(.*):([0-9]*)', re.DOTALL)

    match = _portprog.fullmatch(host)
    if match:
        host, port = match.groups()
        if port:
            return host, port
    return host, None


def splitnport(host, defport=-1):
    warnings.warn("urllib.parse.splitnport() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitnport(host, defport)


def _splitnport(host, defport=-1):
    """Split host and port, returning numeric port.
    Return given default port if no ':' found; defaults to -1.
    Return numerical port if a valid number is found after ':'.
    Return None if ':' but not a valid number."""
    host, delim, port = host.rpartition(':')
    if not delim:
        host = port
    elif port:
        if port.isdigit() and port.isascii():
            nport = int(port)
        else:
            nport = None
        return host, nport
    return host, defport


def splitquery(url):
    warnings.warn("urllib.parse.splitquery() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitquery(url)


def _splitquery(url):
    """splitquery('/path?query') --> '/path', 'query'."""
    path, delim, query = url.rpartition('?')
    if delim:
        return path, query
    return url, None


def splittag(url):
    warnings.warn("urllib.parse.splittag() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splittag(url)


def _splittag(url):
    """splittag('/path#tag') --> '/path', 'tag'."""
    path, delim, tag = url.rpartition('#')
    if delim:
        return path, tag
    return url, None


def splitattr(url):
    warnings.warn("urllib.parse.splitattr() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitattr(url)


def _splitattr(url):
    """splitattr('/path;attr1=value1;attr2=value2;...') ->
        '/path', ['attr1=value1', 'attr2=value2', ...]."""
    words = url.split(';')
    return words[0], words[1:]


def splitvalue(attr):
    warnings.warn("urllib.parse.splitvalue() is deprecated as of 3.8, "
                  "use urllib.parse.parse_qsl() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitvalue(attr)


def _splitvalue(attr):
    """splitvalue('attr=value') --> 'attr', 'value'."""
    attr, delim, value = attr.partition('=')
    return attr, (value if delim else None)


def to_bytes(url):
    warnings.warn("urllib.parse.to_bytes() is deprecated as of 3.8",
                  DeprecationWarning, stacklevel=2)
    return _to_bytes(url)


def _to_bytes(url):
    """to_bytes(u"URL") --> 'URL'."""
    # Most URL schemes require ASCII. If that changes, the conversion
    # can be relaxed.
    # XXX get rid of to_bytes()
    if isinstance(url, str):
        try:
            url = url.encode("ASCII").decode()
        except UnicodeError:
            raise UnicodeError("URL " + repr(url) +
                               " contains non-ASCII characters")
    return url


def unwrap(url):
    """Transform a string like '<URL:scheme://host/path>' into 'scheme://host/path'.

    The string is returned unchanged if it's not a wrapped URL.
    """
    url = str(url).strip()
    if url[:1] == '<' and url[-1:] == '>':
        url = url[1:-1].strip()
    if url[:4] == 'URL:':
        url = url[4:].strip()
    return url


_scheme_letters = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ'
_scheme_chars = _scheme_letters + '0123456789+-.'


def _urlsplit(url, scheme=None, allow_fragments=True):
    url = url.lstrip(_WHATWG_C0_CONTROL_OR_SPACE)
    for char in ('\t', '\r', '\n'):
        url = url.replace(char, '')
    if scheme is not None:
        scheme = scheme.strip(_WHATWG_C0_CONTROL_OR_SPACE)
        for char in ('\t', '\r', '\n'):
            scheme = scheme.replace(char, '')
    netloc = query = fragment = None
    i = url.find(':')
    if i > 0 and url[0] in _scheme_letters:
        for c in url[:i]:
            if c not in _scheme_chars:
                break
        else:
            scheme, url = url[:i].lower(), url[i + 1:]
    if url[:2] == '//':
        end = len(url)
        for c in '/?#':
            pos = url.find(c, 2)
            if pos >= 0:
                end = min(end, pos)
        netloc, url = url[2:end], url[end:]
        _check_netloc(netloc)
    if allow_fragments and '#' in url:
        url, fragment = url.split('#', 1)
    if '?' in url:
        url, query = url.split('?', 1)
    return scheme, netloc, url, query, fragment


def _urlunsplit(scheme, netloc, url, query, fragment):
    if netloc is not None:
        if url and url[:1] != '/':
            url = '/' + url
        url = '//' + netloc + url
    elif url[:2] == '//':
        url = '//' + url
    if scheme:
        url = scheme + ':' + url
    if query is not None:
        url = url + '?' + query
    if fragment is not None:
        url = url + '#' + fragment
    return url


def urldefrag(url):
    """Removes any existing fragment from URL.

    Returns a tuple of the defragmented URL and the fragment.  If
    the URL contained no fragments, the second element is the
    empty string.
    """
    text_input = isinstance(url, str)
    if not text_input:
        url = url.decode('ascii', 'strict') if url else ''
    if '#' in url:
        scheme, netloc, path, query, fragment = _urlsplit(url)
        url = _urlunsplit(scheme, netloc, path, query, None)
        fragment = fragment or ''
    else:
        fragment = ''
    result = DefragResult(url, fragment)
    return result if text_input else result.encode('ascii', 'strict')


def unquote_plus(string, encoding='utf-8', errors='replace'):
    """Like unquote(), but also replace plus signs by spaces, as required for
    unquoting HTML form values.

    unquote_plus('%7e/abc+def') -> '~/abc def'
    """
    string = string.replace('+', ' ')
    return unquote(string, encoding, errors)


_hexdig = '0123456789ABCDEFabcdef'
_hextobyte = None


def unquote_to_bytes(string):
    """Decode percent escapes and return the resulting bytes."""
    if not string:
        # Is it a string-like object?
        string.split
        return b''
    if isinstance(string, str):
        string = string.encode('utf-8')
    bits = string.split(b'%')
    if len(bits) == 1:
        return bytes(string)
    res = bytearray(bits[0])
    append = res.extend
    # Delay the initialization of the table to not waste memory
    # if the function is never called
    global _hextobyte
    if _hextobyte is None:
        _hextobyte = {(a + b).encode(): bytes.fromhex(a + b)
                      for a in _hexdig for b in _hexdig}
    for item in bits[1:]:
        try:
            append(_hextobyte[item[:2]])
            append(item[2:])
        except KeyError:
            append(b'%')
            append(item)
    return bytes(res)


def parse_qs(qs, keep_blank_values=False, strict_parsing=False,
             encoding='utf-8', errors='replace', max_num_fields=None, separator='&'):
    """Parse a query into lists of values grouped by name."""
    result = {}
    for name, value in parse_qsl(qs, keep_blank_values, strict_parsing, encoding=encoding,
                                 errors=errors, max_num_fields=max_num_fields,
                                 separator=separator, _stacklevel=2):
        if name in result:
            result[name].append(value)
        else:
            result[name] = [value]
    return result


def parse_qsl(qs, keep_blank_values=False, strict_parsing=False,
              encoding='utf-8', errors='replace', max_num_fields=None, separator='&', *, _stacklevel=1):
    """Parse a query into name/value pairs in field order."""
    if not separator or not isinstance(separator, (str, bytes)):
        raise ValueError('Separator must be of type string or bytes.')
    text = isinstance(qs, str)
    if text:
        if not isinstance(separator, str):
            separator = str(separator, 'ascii')

        def _unquote(s):
            return unquote_plus(s, encoding=encoding, errors=errors)
    elif qs is None:
        return []
    else:
        # A memoryview may be strided; take its bytes in logical order.
        if isinstance(qs, memoryview):
            qs = bytes(qs)
        try:
            # Latin-1 maps each octet to one character and reads the buffer
            # directly, bypassing __bytes__ on subclasses.
            qs = str(qs, 'latin-1')
        except TypeError:
            if qs:
                raise
            warnings.warn(f"Accepting {type(qs).__name__} objects with "
                          "false value in urllib.parse.parse_qsl() is deprecated as of 3.14",
                          DeprecationWarning, stacklevel=_stacklevel + 1)
            return []
        if isinstance(separator, str):
            separator = bytes(separator, 'ascii')
        separator = str(separator, 'latin-1')

        def _unquote(s):
            return unquote_to_bytes(s.replace('+', ' ').encode('latin-1'))

    if not qs:
        return []
    if max_num_fields is not None and max_num_fields < 1 + qs.count(separator):
        raise ValueError('Max number of fields exceeded')

    result = []
    for field in qs.split(separator):
        if field or strict_parsing:
            name, has_eq, value = field.partition('=')
            if not has_eq and strict_parsing:
                if not text:
                    field = field.encode('latin-1')
                raise ValueError('bad query field: %r' % (field,))
            if value or keep_blank_values:
                name = _unquote(name)
                value = _unquote(value)
                result.append((name, value))
    return result


def clear_cache():
    """Clear internal performance caches. Undocumented; some tests want it."""
    urlsplit.cache_clear()
    _byte_quoter_factory.cache_clear()


_ALWAYS_SAFE = frozenset(b'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
                         b'abcdefghijklmnopqrstuvwxyz'
                         b'0123456789'
                         b'_.-~')
_ALWAYS_SAFE_BYTES = bytes(_ALWAYS_SAFE)


class _Quoter(dict):
    """A mapping from bytes numbers (in range(0,256)) to strings.

    String values are percent-encoded byte values, unless the key < 128, and
    in either of the specified safe set, or the always safe set.
    """
    # Keeps a cache internally, via __missing__, for efficiency (lookups
    # of cached keys don't call Python code at all).
    def __init__(self, safe):
        """safe: bytes object."""
        self.safe = _ALWAYS_SAFE | frozenset(safe)

    def __repr__(self):
        return f"<Quoter {dict(self)!r}>"

    def __missing__(self, b):
        # Handle a cache miss. Store quoted string in cache and return.
        res = chr(b) if b in self.safe else '%{:02X}'.format(b)
        self[b] = res
        return res

def quote(string, safe='/', encoding=None, errors=None):
    """quote('abc def') -> 'abc%20def'

    Each part of a URL, e.g. the path info, the query, etc., has a
    different set of reserved characters that must be quoted. The
    quote function offers a cautious (not minimal) way to quote a
    string for most of these parts.

    RFC 3986 Uniform Resource Identifier (URI): Generic Syntax lists
    the following (un)reserved characters.

    unreserved    = ALPHA / DIGIT / "-" / "." / "_" / "~"
    reserved      = gen-delims / sub-delims
    gen-delims    = ":" / "/" / "?" / "#" / "[" / "]" / "@"
    sub-delims    = "!" / "$" / "&" / "'" / "(" / ")"
                  / "*" / "+" / "," / ";" / "="

    Each of the reserved characters is reserved in some component of a URL,
    but not necessarily in all of them.

    The quote function %-escapes all characters that are neither in the
    unreserved chars ("always safe") nor the additional chars set via the
    safe arg.

    The default for the safe arg is '/'. The character is reserved, but in
    typical usage the quote function is being called on a path where the
    existing slash characters are to be preserved.

    Python 3.7 updates from using RFC 2396 to RFC 3986 to quote URL strings.
    Now, "~" is included in the set of unreserved characters.

    string and safe may be either str or bytes objects. encoding and errors
    must not be specified if string is a bytes object.

    The optional encoding and errors parameters specify how to deal with
    non-ASCII characters, as accepted by the str.encode method.
    By default, encoding='utf-8' (characters are encoded with UTF-8), and
    errors='strict' (unsupported characters raise a UnicodeEncodeError).
    """
    if isinstance(string, str):
        if not string:
            return string
        if encoding is None:
            encoding = 'utf-8'
        if errors is None:
            errors = 'strict'
        string = string.encode(encoding, errors)
    else:
        if encoding is not None:
            raise TypeError("quote() doesn't support 'encoding' for bytes")
        if errors is not None:
            raise TypeError("quote() doesn't support 'errors' for bytes")
    return quote_from_bytes(string, safe)

def quote_plus(string, safe='', encoding=None, errors=None):
    """Like quote(), but also replace ' ' with '+', as required for quoting
    HTML form values. Plus signs in the original string are escaped unless
    they are included in safe. It also does not have safe default to '/'.
    """
    # Check if ' ' in string, where string may either be a str or bytes.  If
    # there are no spaces, the regular quote will produce the right answer.
    if ((isinstance(string, str) and ' ' not in string) or
        (isinstance(string, bytes) and b' ' not in string)):
        return quote(string, safe, encoding, errors)
    if isinstance(safe, str):
        space = ' '
    else:
        space = b' '
    string = quote(string, safe + space, encoding, errors)
    return string.replace(' ', '+')

# Expectation: A typical program is unlikely to create more than 5 of these.
@functools.lru_cache
def _byte_quoter_factory(safe):
    return _Quoter(safe).__getitem__

def quote_from_bytes(bs, safe='/'):
    """Like quote(), but accepts a bytes object rather than a str, and does
    not perform string-to-bytes encoding.  It always returns an ASCII string.
    quote_from_bytes(b'abc def\x3f') -> 'abc%20def%3f'
    """
    if not isinstance(bs, (bytes, bytearray)):
        raise TypeError("quote_from_bytes() expected bytes")
    if not bs:
        return ''
    if isinstance(safe, str):
        # Normalize 'safe' by converting to bytes and removing non-ASCII chars
        safe = safe.encode('ascii', 'ignore')
    else:
        # List comprehensions are faster than generator expressions.
        safe = bytes([c for c in safe if c < 128])
    if not bs.rstrip(_ALWAYS_SAFE_BYTES + safe):
        return bs.decode()
    quoter = _byte_quoter_factory(safe)
    if (bs_len := len(bs)) < 200_000:
        return ''.join(map(quoter, bs))
    else:
        # This saves memory - https://github.com/python/cpython/issues/95865
        chunk_size = math.isqrt(bs_len)
        chunks = [''.join(map(quoter, bs[i:i+chunk_size]))
                  for i in range(0, bs_len, chunk_size)]
        return ''.join(chunks)


def urlencode(query, doseq=False, safe='', encoding=None, errors=None, quote_via=quote_plus):
    """Encode a dict or sequence of two-element tuples into a URL query string.

    If any values in the query arg are sequences and doseq is true, each
    sequence element is converted to a separate parameter.

    If the query arg is a sequence of two-element tuples, the order of the
    parameters in the output will match the order of parameters in the
    input.

    The components of a query arg may each be either a string or a bytes type.

    The safe, encoding, and errors parameters are passed down to the function
    specified by quote_via (encoding and errors only if a component is a str).
    """
    return _imre_urlencode(query, doseq, safe, encoding, errors, quote_via)


def unquote(string, encoding='utf-8', errors='replace'):
    """Replace %xx escapes by their single-character equivalent. The optional
    encoding and errors parameters specify how to decode percent-encoded
    sequences into Unicode characters, as accepted by the bytes.decode()
    method.
    By default, percent-encoded sequences are decoded with UTF-8, and invalid
    sequences are replaced by a placeholder character.

    unquote('abc%20def') -> 'abc def'.
    """
    return _imre_unquote(string, encoding, errors)


def urljoin(base, url, allow_fragments=True):
    """Join a base URL and a possibly relative URL to form an absolute
    interpretation of the latter."""
    return _imre_urljoin(base, url, allow_fragments)


def _noop(value):
    return value


def _encode_result(value):
    return value.encode('ascii', 'strict')


def _coerce_args(*args):
    text = isinstance(args[0], str)
    for arg in args[1:]:
        if arg and isinstance(arg, str) != text:
            raise TypeError("Cannot mix str and non-str arguments")
    if text:
        return args + (_noop,)
    return tuple(arg.decode('ascii', 'strict') if arg else '' for arg in args) + (_encode_result,)


def _recompose_components(components, with_params):
    fields = _coerce_args(*components)
    if with_params:
        scheme, netloc, path, params, query, fragment, coerce = fields
    else:
        scheme, netloc, path, query, fragment, coerce = fields
        params = None
    if not netloc:
        if scheme and scheme in uses_netloc and (not path or path[:1] == '/'):
            netloc = ''
        else:
            netloc = None
    if params:
        path = '%s;%s' % (path, params)
    return coerce(_urlunsplit(scheme or None, netloc, path, query or None, fragment or None))


def urlunparse(components):
    """Put a parsed URL back together again.  This may result in a
    slightly different, but equivalent URL, if the URL that was parsed
    originally had redundant delimiters, e.g. a ? with an empty query
    (the draft states that these are equivalent)."""
    return _imre_urlunparse(components, _recompose_components)


def urlunsplit(components):
    """Combine the elements of a tuple as returned by urlsplit() into a
    complete URL as a string. The data argument can be any five-item iterable.
    This may result in a slightly different, but equivalent URL, if the URL that
    was parsed originally had unnecessary delimiters (for example, a ? with an
    empty query; the RFC states that these are equivalent)."""
    return _imre_urlunsplit(components, _recompose_components)
