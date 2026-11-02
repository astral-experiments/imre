def _messageLength(self) -> int:
    return len(self._headers)

def _messageIter(self):
    for field, value in self._headers:
        yield field

def _messageContains(self, name: str) -> bool:
    name = name.lower()
    return any(field.lower() == name for field, value in self._headers)

Message.__len__ = _messageLength
Message.__iter__ = _messageIter
Message.__contains__ = _messageContains


# MIME parameter handling from CPython 3.14.5 Lib/email/message.py and utils.py.

import re as _re

def _parseparam(s):
    # RDM This might be a Header, so for now stringify it.
    s = ';' + str(s)
    plist = []
    start = 0
    while s.find(';', start) == start:
        start += 1
        end = s.find(';', start)
        ind, diff = start, 0
        while end > 0:
            diff += s.count('"', ind, end) - s.count('\\"', ind, end)
            if diff % 2 == 0:
                break
            end, ind = ind, s.find(';', end + 1)
        if end < 0:
            end = len(s)
        i = s.find('=', start, end)
        if i == -1:
            f = s[start:end]
        else:
            f = s[start:i].rstrip().lower() + '=' + s[i+1:end].lstrip()
        plist.append(f.strip())
        start = end
    return plist


def _mime_unquote(str):
    """Remove quotes from a string."""
    if len(str) > 1:
        if str.startswith('"') and str.endswith('"'):
            return str[1:-1].replace('\\\\', '\\').replace('\\"', '"')
        if str.startswith('<') and str.endswith('>'):
            return str[1:-1]
    return str


def _mime_quote(str):
    """Quote backslashes and double quotes in a quoted parameter."""
    return str.replace('\\', '\\\\').replace('"', '\\"')


def _mime_decode_rfc2231(s):
    """Decode string according to RFC 2231"""
    parts = s.split("'", 2)
    if len(parts) <= 2:
        return None, None, s
    return parts


_rfc2231_continuation = _re.compile('^(?P<name>\\w+)\\*((?P<num>[0-9]+)\\*?)?$', _re.ASCII)

def _decode_mime_params(params):
    """Decode RFC 2231 parameters and numbered continuations."""
    new_params = [params[0]]
    # Map parameter's name to a list of continuations.  The values are a
    # 3-tuple of the continuation number, the string value, and a flag
    # specifying whether a particular segment is %-encoded.
    rfc2231_params = {}
    for name, value in params[1:]:
        encoded = name.endswith('*')
        value = _mime_unquote(value)
        mo = _rfc2231_continuation.match(name)
        if mo:
            name, num = mo.group('name', 'num')
            if num is not None:
                num = int(num)
            rfc2231_params.setdefault(name, []).append((num, value, encoded))
        else:
            new_params.append((name, '"%s"' % _mime_quote(value)))
    if rfc2231_params:
        for name, continuations in rfc2231_params.items():
            value = []
            extended = False
            # Sort by number, treating None as 0 if there is no 0,
            # and ignore it if there is already a 0.
            has_zero = any(x[0] == 0 for x in continuations)
            if has_zero:
                continuations = [x for x in continuations if x[0] is not None]
            else:
                continuations = [(x[0] or 0, x[1], x[2]) for x in continuations]
            continuations.sort(key=lambda x: x[0])
            # And now append all values in numerical order, converting
            # %-encodings for the encoded segments.  If any of the
            # continuation names ends in a *, then the entire string, after
            # decoding segments and concatenating, must have the charset and
            # language specifiers at the beginning of the string.
            for num, s, encoded in continuations:
                if encoded:
                    # Decode as "latin-1", so the characters in s directly
                    # represent the percent-encoded octet values.
                    # collapse_rfc2231_value treats this as an octet sequence.
                    from urllib.parse import unquote
                    s = unquote(s, encoding="latin-1")
                    extended = True
                value.append(s)
            value = _mime_quote("".join(value))
            if extended:
                charset, language, value = _mime_decode_rfc2231(value)
                new_params.append((name, (charset, language, '"%s"' % value)))
            else:
                new_params.append((name, '"%s"' % value))
    return new_params


def _unquotevalue(value):
    # This is different than utils.collapse_rfc2231_value() because it doesn't
    # try to convert the value to a unicode.  Message.get_param() and
    # Message.get_params() are both currently defined to return the tuple in
    # the face of RFC 2231 parameters.
    if isinstance(value, tuple):
        return value[0], value[1], _mime_unquote(value[2])
    else:
        return _mime_unquote(value)


def _get_params_preserve(self, failobj, header):
    # Like get_params() but preserves the quoting of values.  BAW:
    # should this be part of the public interface?
    missing = object()
    value = self.get(header, missing)
    if value is missing:
        return failobj
    params = []
    for p in _parseparam(value):
        try:
            name, val = p.split('=', 1)
            name = name.strip()
            val = val.strip()
        except ValueError:
            # Must have been a bare attribute
            name = p.strip()
            val = ''
        params.append((name, val))
    params = _decode_mime_params(params)
    return params


def get_params(self, failobj=None, header='content-type', unquote=True):
    """Return the parameter pairs of a message header."""
    missing = object()
    params = self._get_params_preserve(missing, header)
    if params is missing:
        return failobj
    if unquote:
        return [(k, _unquotevalue(v)) for k, v in params]
    else:
        return params


Message._get_params_preserve = _get_params_preserve
Message.get_params = get_params
del get_params
