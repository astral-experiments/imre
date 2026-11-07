# Linux epoll selector from CPython 3.14.5, Lib/selectors.py.
import select


def _fileobj_to_fd(fileobj):
    """Return a file descriptor from a file object.

    Parameters:
    fileobj -- file object or file descriptor

    Returns:
    corresponding file descriptor

    Raises:
    ValueError if the object is invalid
    """
    if isinstance(fileobj, int):
        fd = fileobj
    else:
        try:
            fd = int(fileobj.fileno())
        except (AttributeError, TypeError, ValueError):
            raise ValueError("Invalid file object: "
                             "{!r}".format(fileobj)) from None
    if fd < 0:
        raise ValueError("Invalid file descriptor: {}".format(fd))
    return fd


class _BaseSelectorImpl(BaseSelector):
    """Base selector implementation."""

    def __init__(self):
        # this maps file descriptors to keys
        self._fd_to_key = {}
        # read-only mapping returned by get_map()
        self._map = _SelectorMapping(self)

    def _fileobj_lookup(self, fileobj):
        """Return a file descriptor from a file object.

        This wraps _fileobj_to_fd() to do an exhaustive search in case
        the object is invalid but we still have it in our map.  This
        is used by unregister() so we can unregister an object that
        was previously registered even if it is closed.  It is also
        used by _SelectorMapping.
        """
        try:
            return _fileobj_to_fd(fileobj)
        except ValueError:
            # Do an exhaustive search.
            for key in self._fd_to_key.values():
                if key.fileobj is fileobj:
                    return key.fd
            # Raise ValueError after all.
            raise

    def register(self, fileobj, events, data=None):
        if (not events) or (events & ~(EVENT_READ | EVENT_WRITE)):
            raise ValueError("Invalid events: {!r}".format(events))

        key = SelectorKey(fileobj, self._fileobj_lookup(fileobj), events, data)

        if key.fd in self._fd_to_key:
            raise KeyError("{!r} (FD {}) is already registered"
                           .format(fileobj, key.fd))

        self._fd_to_key[key.fd] = key
        return key

    def unregister(self, fileobj):
        try:
            key = self._fd_to_key.pop(self._fileobj_lookup(fileobj))
        except KeyError:
            raise KeyError("{!r} is not registered".format(fileobj)) from None
        return key

    def modify(self, fileobj, events, data=None):
        try:
            key = self._fd_to_key[self._fileobj_lookup(fileobj)]
        except KeyError:
            raise KeyError("{!r} is not registered".format(fileobj)) from None
        if events != key.events:
            self.unregister(fileobj)
            key = self.register(fileobj, events, data)
        elif data != key.data:
            # Use a shortcut to update the data.
            key = key._replace(data=data)
            self._fd_to_key[key.fd] = key
        return key

    def close(self):
        self._fd_to_key.clear()
        self._map = None

    def get_map(self):
        return self._map


class _PollLikeSelector(_BaseSelectorImpl):
    """Base class shared between poll, epoll and devpoll selectors."""
    _selector_cls = None
    _EVENT_READ = None
    _EVENT_WRITE = None

    def __init__(self):
        super().__init__()
        self._selector = self._selector_cls()

    def register(self, fileobj, events, data=None):
        key = super().register(fileobj, events, data)
        poller_events = ((events & EVENT_READ and self._EVENT_READ)
                         | (events & EVENT_WRITE and self._EVENT_WRITE) )
        try:
            self._selector.register(key.fd, poller_events)
        except:
            super().unregister(fileobj)
            raise
        return key

    def unregister(self, fileobj):
        key = super().unregister(fileobj)
        try:
            self._selector.unregister(key.fd)
        except OSError:
            # This can happen if the FD was closed since it
            # was registered.
            pass
        return key

    def modify(self, fileobj, events, data=None):
        try:
            key = self._fd_to_key[self._fileobj_lookup(fileobj)]
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None

        changed = False
        if events != key.events:
            selector_events = ((events & EVENT_READ and self._EVENT_READ)
                               | (events & EVENT_WRITE and self._EVENT_WRITE))
            try:
                self._selector.modify(key.fd, selector_events)
            except:
                super().unregister(fileobj)
                raise
            changed = True
        if data != key.data:
            changed = True

        if changed:
            key = key._replace(events=events, data=data)
            self._fd_to_key[key.fd] = key
        return key


_epoll = getattr(select, 'epoll', None)
if callable(_epoll):
    _NOT_EPOLLIN = ~select.EPOLLIN
    _NOT_EPOLLOUT = ~select.EPOLLOUT


    class EpollSelector(_PollLikeSelector):
        """Epoll-based selector."""
        _selector_cls = _epoll
        _EVENT_READ = select.EPOLLIN
        _EVENT_WRITE = select.EPOLLOUT

        def fileno(self):
            return self._selector.fileno()

        def select(self, timeout=None):
            if timeout is None:
                timeout = -1
            elif timeout <= 0:
                timeout = 0
            else:
                # epoll_wait() has a resolution of 1 millisecond, round away
                # from zero to wait *at least* timeout seconds.
                timeout = math.ceil(timeout * 1e3) * 1e-3

            # epoll_wait() expects `maxevents` to be greater than zero;
            # we want to make sure that `select()` can be called when no
            # FD is registered.
            max_ev = len(self._fd_to_key) or 1

            ready = []
            try:
                fd_event_list = self._selector.poll(timeout, max_ev)
            except InterruptedError:
                return ready

            fd_to_key = self._fd_to_key
            for fd, event in fd_event_list:
                key = fd_to_key.get(fd)
                if key:
                    events = ((event & _NOT_EPOLLIN and EVENT_WRITE)
                              | (event & _NOT_EPOLLOUT and EVENT_READ))
                    ready.append((key, events & key.events))
            return ready

        def close(self):
            self._selector.close()
            super().close()
