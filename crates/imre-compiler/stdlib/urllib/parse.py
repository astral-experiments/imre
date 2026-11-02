# URL helpers from CPython 3.14.5 Lib/urllib/parse.py (PSF-2.0).

from collections import namedtuple
import functools
import re
import warnings
from urllib.parse import unquote


@functools.lru_cache(typed=True)
def urlsplit(url, scheme='', allow_fragments=True):
    return _imre_urlsplit(url, scheme, allow_fragments)


def urlparse(url, scheme='', allow_fragments=True):
    return _imre_urlparse(url, scheme, allow_fragments)


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


def _defrag_parts(url):
    # An empty authority or query still has a delimiter. Keep it until the
    # fields are reassembled, and honor string operations on subclasses.
    scheme = netloc = query = None
    fragment = ''
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
    if '#' in url:
        url, fragment = url.split('#', 1)
    if '?' in url:
        url, query = url.split('?', 1)
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
    return url, fragment or ''


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
        url = url.lstrip(_WHATWG_C0_CONTROL_OR_SPACE)
        for char in ('\t', '\r', '\n'):
            url = url.replace(char, '')
        url, fragment = _defrag_parts(url)
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
