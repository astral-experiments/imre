"""Read the RFC 822 headers and description of an installed distribution."""
import re
from email.message import Message as EmailMessage
from email.parser import HeaderParser
from _imre_metadata import dedent


class Message(EmailMessage):
    # Fields allowed to occur multiple times in PEP 566's JSON representation.
    _multiple = {
        'classifier', 'obsoletes-dist', 'platform', 'project-url', 'provides-dist',
        'provides-extra', 'requires-dist', 'requires-external', 'supported-platform', 'dynamic',
    }

    def __init__(self, text):
        super().__init__()
        parsed = HeaderParser().parsestr(text)
        for name, value in parsed.raw_items():
            if '\n' in value:
                value = dedent(' ' * 8 + value)
            self._headers.append((name, value))
        # Preserve the description text before email's payload charset conversion.
        if parsed._payload:
            self._headers.append(('Description', parsed._payload))

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
