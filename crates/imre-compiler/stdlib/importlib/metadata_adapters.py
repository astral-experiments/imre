"""Read the RFC 822 headers and description of an installed distribution."""
import re
from email.message import Message as EmailMessage, _valid_field_name
from _imre_metadata import dedent


class Message(EmailMessage):
    # Fields allowed to occur multiple times in PEP 566's JSON representation.
    _multiple = {
        'classifier', 'obsoletes-dist', 'platform', 'project-url', 'provides-dist',
        'provides-extra', 'requires-dist', 'requires-external', 'supported-platform', 'dynamic',
    }

    def __init__(self, text):
        super().__init__()
        # Only CR and LF delimit RFC 822 lines. Other Unicode line separators are data.
        lines = re.findall(r'[^\r\n]*(?:\r\n|\r|\n)|[^\r\n]+$', text)
        i = 0
        envelope = ''
        while i < len(lines):
            line = lines[i]
            # The email parser treats a trailing, non-initial Unix envelope as body.
            if line.startswith('From '):
                envelope = line if i else ''
                i += 1
                continue
            if line.startswith((' ', '\t')):
                envelope = ''
                i += 1
                continue
            if line in ('\n', '\r', '\r\n'):
                i += 1
                break
            name, colon, value = line.partition(':')
            if not colon or (name and not _valid_field_name(name)):
                break
            envelope = ''
            i += 1
            while i < len(lines) and lines[i].startswith((' ', '\t')):
                value += lines[i]
                i += 1
            if name:
                value = value.lstrip(' \t\r\n').rstrip('\r\n')
                if '\n' in value:
                    value = dedent(' ' * 8 + value)
                self._headers.append((name, value))
        if envelope or i < len(lines):
            self._headers.append(('Description', envelope + ''.join(lines[i:])))

    def __getitem__(self, name):
        value = self.get(name)
        if value is None:
            import warnings
            warnings.warn('Implicit None on return values is deprecated and will raise KeyErrors.',
                          DeprecationWarning, stacklevel=2)
        return value

    def keys(self):
        return list(self)

    def values(self):
        return [value for name, value in self._headers]

    @property
    def json(self):
        result = {}
        for key in self:
            name = key.lower()
            value = self.get_all(key) if name in self._multiple else self[key]
            if name == 'keywords':
                value = re.split(r'\s+', value)
            result[name.replace('-', '_')] = value
        return result
