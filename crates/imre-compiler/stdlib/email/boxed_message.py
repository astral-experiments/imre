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
