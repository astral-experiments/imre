from contextlib import contextmanager, suppress


def check():
    ignore_lookup = suppress(LookupError)
    with ignore_lookup as entered:
        assert entered is None
        raise KeyError('suppressed')
    try:
        with ignore_lookup:
            raise ValueError('propagated')
    except ValueError as error:
        assert error.args == ('propagated',)
    else:
        raise AssertionError('suppressed an unrelated exception')

    events = []

    @contextmanager
    def track(x, /, *args, flag=True, **kwds):
        """Track entry and exit."""
        events.append(('enter', x, args, flag, kwds))
        try:
            yield x
        finally:
            events.append('exit')

    assert track.__wrapped__.__name__ == track.__name__ == 'track'
    assert track.__doc__ == 'Track entry and exit.'
    cm = track(7, 8, 9, flag=False, extra=10)
    assert events == []
    with cm as value:
        assert value == 7
    assert events == [('enter', 7, (8, 9), False, {'extra': 10}), 'exit']
    try:
        with cm:
            pass
    except AttributeError as error:
        print(str(error))
    else:
        raise AssertionError('reused generator context')

    @track(5)
    def decorated():
        return 23

    assert decorated() == decorated() == 23

    @contextmanager
    def nested(label):
        with track(label):
            try:
                yield
            finally:
                events.append(label)

    def exit_method(label):
        manager = nested(label)
        manager.__enter__()
        return manager.__exit__

    def discard(value):
        pass

    def check_transients():
        assert exit_method('truth') and events[-2:] == ['truth', 'exit']
        assert (discard(exit_method('call')), events[-2:]) == (None, ['call', 'exit'])

    with nested('outer'):
        check_transients()
    assert events[-2:] == ['outer', 'exit']

    for exc in (ValueError('body'), RuntimeError('body'), StopIteration('body')):
        try:
            with track(1):
                raise exc
        except BaseException as error:
            assert error is exc
        else:
            raise AssertionError('suppressed body exception')

    class Handler:
        def __iter__(self):
            return self

        def __next__(self):
            return None

        def throw(self, error):
            events.append(error)
            raise StopIteration

    @contextmanager
    def consume():
        yield from Handler()

    marker = ValueError('caught')
    with consume():
        raise marker
    assert events[-1] is marker

    @contextmanager
    def replace():
        try:
            yield
        finally:
            raise KeyError('replaced')

    try:
        with replace():
            raise marker
    except KeyError as error:
        assert error.args == ('replaced',)
    else:
        raise AssertionError('lost generator exception')

    @contextmanager
    def empty():
        if False:
            yield

    @contextmanager
    def twice():
        try:
            try:
                yield
            finally:
                yield
        finally:
            events.append('closed')

    for make, exceptional, message in ((empty, False, "generator didn't yield"),
                                       (twice, False, "generator didn't stop"),
                                       (twice, True, "generator didn't stop after throw()")):
        before = len(events)
        try:
            with make():
                if exceptional:
                    raise marker
        except RuntimeError as error:
            assert str(error) == message
        else:
            raise AssertionError('generator protocol error was missed')
        assert events[before:] == (['closed'] if make is twice else [])


if __name__ == '__main__':
    check()
