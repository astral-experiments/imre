"""Integer descriptor selectors for Imre's POSIX poll backend."""
from collections import namedtuple
import select

EVENT_READ = 1
EVENT_WRITE = 2
SelectorKey = namedtuple('SelectorKey', ('fileobj', 'fd', 'events', 'data'))

class PollSelector:
    _poll: select.poll
    _keys: dict[int, SelectorKey]
    _closed: bool

    def __init__(self) -> None:
        self._poll = select.poll()
        self._keys = {}
        self._closed = False

    def register(self, fileobj: int, events: int, data: object = None) -> SelectorKey:
        if not events or events & ~(EVENT_READ | EVENT_WRITE):
            raise ValueError(f'Invalid events: {events!r}')
        if fileobj < 0:
            raise ValueError(f'Invalid file descriptor: {fileobj}')
        if fileobj in self._keys:
            raise KeyError(f'{fileobj!r} (FD {fileobj}) is already registered')
        key = SelectorKey(fileobj, fileobj, events, data)
        self._poll.register(fileobj, _mask(events))
        self._keys[fileobj] = key
        return key

    def get_key(self, fileobj: int) -> SelectorKey:
        if self._closed:
            raise RuntimeError('Selector is closed')
        return self._lookup(fileobj)

    def _lookup(self, fileobj: int) -> SelectorKey:
        if fileobj < 0:
            raise ValueError(f'Invalid file descriptor: {fileobj}')
        if fileobj not in self._keys:
            raise KeyError(f'{fileobj!r} is not registered')
        return self._keys[fileobj]

    def unregister(self, fileobj: int) -> SelectorKey:
        key = self._lookup(fileobj)
        self._poll.unregister(fileobj)
        del self._keys[fileobj]
        return key

    def modify(self, fileobj: int, events: int, data: object = None) -> SelectorKey:
        key = self._lookup(fileobj)
        if events != key.events:
            self._poll.modify(fileobj, _mask(events))
        if events != key.events or data != key.data:
            key = SelectorKey(fileobj, fileobj, events, data)
            self._keys[fileobj] = key
        return key

    def select(self, timeout: float | None = None) -> list[tuple[SelectorKey, int]]:
        milliseconds: float | None = None
        if timeout is not None:
            milliseconds = 0.0 if timeout <= 0.0 else timeout * 1000.0
        ready: list[tuple[SelectorKey, int]] = []
        for fd, event in self._poll.poll(milliseconds):
            if fd not in self._keys:
                continue
            key = self._keys[fd]
            events = 0
            if event & ~select.POLLIN:
                events |= EVENT_WRITE
            if event & ~select.POLLOUT:
                events |= EVENT_READ
            ready.append((key, events & key.events))
        return ready

    def close(self) -> None:
        self._keys.clear()
        self._closed = True

    def __enter__(self) -> 'PollSelector':
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.close()


def _mask(events: int) -> int:
    mask = 0
    if events & EVENT_READ:
        mask |= select.POLLIN
    if events & EVENT_WRITE:
        mask |= select.POLLOUT
    return mask

# As in CPython's selectors.py, use poll when no epoll, kqueue or devpoll
# constructor is available in the select module.
DefaultSelector = PollSelector
