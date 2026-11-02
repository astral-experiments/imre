# Structured results from CPython 3.14.5 Lib/urllib/parse.py (PSF-2.0).

from collections import namedtuple


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
